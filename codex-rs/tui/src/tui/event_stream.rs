//! Event stream plumbing for the TUI.
//!
//! - [`EventBroker`] holds the shared crossterm stream so multiple callers reuse the same
//!   input source and can drop/recreate it on pause/resume without rebuilding consumers.
//! - [`TuiEventStream`] wraps a draw event subscription plus the shared [`EventBroker`] and maps crossterm
//!   events into [`TuiEvent`].
//! - [`EventSource`] abstracts the underlying event producer; the real implementation is
//!   [`CrosstermEventSource`] and tests can swap in [`FakeEventSource`].
//!
//! The motivation for dropping/recreating the crossterm event stream is to enable the TUI to fully relinquish stdin.
//! If the stream is not dropped, it will continue to read from stdin even if it is not actively being polled
//! (due to how crossterm's EventStream is implemented), potentially stealing input from other processes reading stdin,
//! like terminal text editors. This race can cause missed input or capturing terminal query responses (for example, OSC palette/size queries)
//! that the other process expects to read. Stopping polling, instead of dropping the stream, is only sufficient when the
//! pause happens before the stream enters a pending state; otherwise the crossterm reader thread may keep reading
//! from stdin, so the safer approach is to drop and recreate the event stream when we need to hand off the terminal.
//!
//! See https://ratatui.rs/recipes/apps/spawn-vim/ and https://www.reddit.com/r/rust/comments/1f3o33u/myterious_crossterm_input_after_running_vim for more details.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::task::Context;
use std::task::Poll;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use crossterm::event::Event;
use gilrs::Axis;
use gilrs::Button;
use gilrs::EventType;
use gilrs::Gilrs;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio_stream::Stream;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::WatchStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

use super::GamepadAction;
use super::TuiEvent;

const GAMEPAD_POLL_INTERVAL: Duration = Duration::from_millis(16);
const GAMEPAD_REPEAT_DELAY: Duration = Duration::from_millis(250);
const GAMEPAD_MIN_REPEAT_INTERVAL: Duration = Duration::from_millis(60);
const GAMEPAD_MAX_REPEAT_INTERVAL: Duration = Duration::from_millis(180);
const NAV_STICK_DEADZONE: f32 = 0.45;
const SCROLL_STICK_DEADZONE: f32 = 0.2;
const SHOULDER_HOLD_DURATION: Duration = Duration::from_secs(2);

/// Result type produced by an event source.
pub type EventResult = std::io::Result<Event>;

/// Abstraction over a source of terminal events. Allows swapping in a fake for tests.
/// Value in production is [`CrosstermEventSource`].
pub trait EventSource: Send + 'static {
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<EventResult>>;
}

/// Shared crossterm input state for all [`TuiEventStream`] instances. A single crossterm EventStream
/// is reused so all streams still see the same input source.
///
/// This intermediate layer enables dropping/recreating the underlying EventStream (pause/resume) without rebuilding consumers.
pub struct EventBroker<S: EventSource = CrosstermEventSource> {
    state: Mutex<EventBrokerState<S>>,
    resume_events_tx: watch::Sender<()>,
}

/// Tracks state of underlying [`EventSource`].
enum EventBrokerState<S: EventSource> {
    Paused,     // Underlying event source (i.e., crossterm EventStream) dropped
    Start,      // A new event source will be created on next poll
    Running(S), // Event source is currently running
}

impl<S: EventSource + Default> EventBrokerState<S> {
    /// Return the running event source, starting it if needed; None when paused.
    fn active_event_source_mut(&mut self) -> Option<&mut S> {
        match self {
            EventBrokerState::Paused => None,
            EventBrokerState::Start => {
                *self = EventBrokerState::Running(S::default());
                match self {
                    EventBrokerState::Running(events) => Some(events),
                    EventBrokerState::Paused | EventBrokerState::Start => unreachable!(),
                }
            }
            EventBrokerState::Running(events) => Some(events),
        }
    }
}

impl<S: EventSource + Default> EventBroker<S> {
    pub fn new() -> Self {
        let (resume_events_tx, _resume_events_rx) = watch::channel(());
        Self {
            state: Mutex::new(EventBrokerState::Start),
            resume_events_tx,
        }
    }

    /// Drop the underlying event source
    pub fn pause_events(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *state = EventBrokerState::Paused;
    }

    /// Create a new instance of the underlying event source
    pub fn resume_events(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *state = EventBrokerState::Start;
        let _ = self.resume_events_tx.send(());
    }

    /// Subscribe to a notification that fires whenever [`Self::resume_events`] is called.
    ///
    /// This is used to wake `poll_crossterm_event` when it is paused and waiting for the
    /// underlying crossterm stream to be recreated.
    pub fn resume_events_rx(&self) -> watch::Receiver<()> {
        self.resume_events_tx.subscribe()
    }
}

/// Real crossterm-backed event source.
pub struct CrosstermEventSource(pub crossterm::event::EventStream);

impl Default for CrosstermEventSource {
    fn default() -> Self {
        Self(crossterm::event::EventStream::new())
    }
}

impl EventSource for CrosstermEventSource {
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<EventResult>> {
        Pin::new(&mut self.get_mut().0).poll_next(cx)
    }
}

/// TuiEventStream is a struct for reading TUI events (draws and user input).
/// Each instance has its own draw subscription (the draw channel is broadcast, so
/// multiple receivers are fine), while crossterm input is funneled through a
/// single shared [`EventBroker`] because crossterm uses a global stdin reader and
/// does not support fan-out. Multiple TuiEventStream instances can exist during the app lifetime
/// (for nested or sequential screens), but only one should be polled at a time,
/// otherwise one instance can consume ("steal") input events and the other will miss them.
pub struct TuiEventStream<S: EventSource + Default + Unpin = CrosstermEventSource> {
    broker: Arc<EventBroker<S>>,
    draw_stream: BroadcastStream<()>,
    resume_stream: WatchStream<()>,
    gamepad_rx: mpsc::UnboundedReceiver<TuiEvent>,
    terminal_focused: Arc<AtomicBool>,
    poll_slot: u8,
    #[cfg(unix)]
    suspend_context: crate::tui::job_control::SuspendContext,
    #[cfg(unix)]
    alt_screen_active: Arc<AtomicBool>,
}

impl<S: EventSource + Default + Unpin> TuiEventStream<S> {
    pub fn new(
        broker: Arc<EventBroker<S>>,
        draw_rx: broadcast::Receiver<()>,
        terminal_focused: Arc<AtomicBool>,
        #[cfg(unix)] suspend_context: crate::tui::job_control::SuspendContext,
        #[cfg(unix)] alt_screen_active: Arc<AtomicBool>,
    ) -> Self {
        let resume_stream = WatchStream::from_changes(broker.resume_events_rx());
        Self {
            broker,
            draw_stream: BroadcastStream::new(draw_rx),
            resume_stream,
            gamepad_rx: spawn_gamepad_event_stream(),
            terminal_focused,
            poll_slot: 0,
            #[cfg(unix)]
            suspend_context,
            #[cfg(unix)]
            alt_screen_active,
        }
    }

    /// Poll the shared crossterm stream for the next mapped `TuiEvent`.
    ///
    /// This skips events we don't use (mouse events, etc.) and keeps polling until it yields
    /// a mapped event, hits `Pending`, or sees EOF/error. When the broker is paused, it drops
    /// the underlying stream and returns `Pending` to fully release stdin.
    pub fn poll_crossterm_event(&mut self, cx: &mut Context<'_>) -> Poll<Option<TuiEvent>> {
        // Some crossterm events map to None (e.g. FocusLost, mouse); loop so we keep polling
        // until we return a mapped event, hit Pending, or see EOF/error.
        loop {
            let poll_result = {
                let mut state = self
                    .broker
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let events = match state.active_event_source_mut() {
                    Some(events) => events,
                    None => {
                        drop(state);
                        // Poll resume_stream so resume_events wakes a stream paused here
                        match Pin::new(&mut self.resume_stream).poll_next(cx) {
                            Poll::Ready(Some(())) => continue,
                            Poll::Ready(None) => return Poll::Ready(None),
                            Poll::Pending => return Poll::Pending,
                        }
                    }
                };
                match Pin::new(events).poll_next(cx) {
                    Poll::Ready(Some(Ok(event))) => Some(event),
                    Poll::Ready(Some(Err(_))) | Poll::Ready(None) => {
                        *state = EventBrokerState::Start;
                        return Poll::Ready(None);
                    }
                    Poll::Pending => {
                        drop(state);
                        // Poll resume_stream so resume_events can wake us even while waiting on stdin
                        match Pin::new(&mut self.resume_stream).poll_next(cx) {
                            Poll::Ready(Some(())) => continue,
                            Poll::Ready(None) => return Poll::Ready(None),
                            Poll::Pending => return Poll::Pending,
                        }
                    }
                }
            };

            if let Some(mapped) = poll_result.and_then(|event| self.map_crossterm_event(event)) {
                return Poll::Ready(Some(mapped));
            }
        }
    }

    /// Poll the draw broadcast stream for the next draw event. Draw events are used to trigger a redraw of the TUI.
    pub fn poll_draw_event(&mut self, cx: &mut Context<'_>) -> Poll<Option<TuiEvent>> {
        match Pin::new(&mut self.draw_stream).poll_next(cx) {
            Poll::Ready(Some(Ok(()))) => Poll::Ready(Some(TuiEvent::Draw)),
            Poll::Ready(Some(Err(BroadcastStreamRecvError::Lagged(_)))) => {
                Poll::Ready(Some(TuiEvent::Draw))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }

    pub fn poll_gamepad_event(&mut self, cx: &mut Context<'_>) -> Poll<Option<TuiEvent>> {
        if !self.terminal_focused.load(Ordering::Relaxed) {
            while let Poll::Ready(Some(_)) = Pin::new(&mut self.gamepad_rx).poll_recv(cx) {}
            return Poll::Pending;
        }
        match Pin::new(&mut self.gamepad_rx).poll_recv(cx) {
            Poll::Ready(Some(event)) => Poll::Ready(Some(event)),
            Poll::Ready(None) | Poll::Pending => Poll::Pending,
        }
    }

    /// Map a crossterm event to a [`TuiEvent`], skipping events we don't use (mouse events, etc.).
    fn map_crossterm_event(&mut self, event: Event) -> Option<TuiEvent> {
        match event {
            Event::Key(key_event) => {
                #[cfg(unix)]
                if crate::tui::job_control::SUSPEND_KEY.is_press(key_event) {
                    let _ = self.suspend_context.suspend(&self.alt_screen_active);
                    return Some(TuiEvent::Draw);
                }
                Some(TuiEvent::Key(key_event))
            }
            Event::Resize(_, _) => Some(TuiEvent::Resize),
            Event::Paste(pasted) => Some(TuiEvent::Paste(pasted)),
            Event::FocusGained => {
                self.terminal_focused.store(true, Ordering::Relaxed);
                crate::terminal_palette::requery_default_colors();
                Some(TuiEvent::Draw)
            }
            Event::FocusLost => {
                self.terminal_focused.store(false, Ordering::Relaxed);
                None
            }
            _ => None,
        }
    }
}

impl<S: EventSource + Default + Unpin> Unpin for TuiEventStream<S> {}

impl<S: EventSource + Default + Unpin> Stream for TuiEventStream<S> {
    type Item = TuiEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let start = self.poll_slot;
        self.poll_slot = (self.poll_slot + 1) % 3;
        for offset in 0..3 {
            match (start + offset) % 3 {
                0 => {
                    if let Poll::Ready(event) = self.poll_crossterm_event(cx) {
                        return Poll::Ready(event);
                    }
                }
                1 => {
                    if let Poll::Ready(event) = self.poll_gamepad_event(cx) {
                        return Poll::Ready(event);
                    }
                }
                _ => {
                    if let Poll::Ready(event) = self.poll_draw_event(cx) {
                        return Poll::Ready(event);
                    }
                }
            }
        }

        Poll::Pending
    }
}

fn spawn_gamepad_event_stream() -> mpsc::UnboundedReceiver<TuiEvent> {
    let (tx, rx) = mpsc::unbounded_channel();
    let builder = thread::Builder::new().name("rolodex-gamepad".to_string());
    if let Err(err) = builder.spawn(move || run_gamepad_event_loop(tx)) {
        tracing::debug!(error = %err, "failed to start gamepad input thread");
    }
    rx
}

fn run_gamepad_event_loop(tx: mpsc::UnboundedSender<TuiEvent>) {
    let mut gilrs = match Gilrs::new() {
        Ok(gilrs) => gilrs,
        Err(err) => {
            tracing::debug!(error = %err, "gamepad support unavailable");
            return;
        }
    };
    let mut state = GamepadState::default();
    while !tx.is_closed() {
        while let Some(event) = gilrs.next_event() {
            state.handle_event(event.event, &tx);
        }
        state.emit_repeats(&tx);
        thread::sleep(GAMEPAD_POLL_INTERVAL);
    }
}

#[derive(Default)]
struct GamepadState {
    left_x: f32,
    left_y: f32,
    right_y: f32,
    dpad_up: bool,
    dpad_down: bool,
    dpad_left: bool,
    dpad_right: bool,
    left_shoulder_pressed_at: Option<Instant>,
    right_shoulder_pressed_at: Option<Instant>,
    left_new_tab_fired: bool,
    right_new_tab_fired: bool,
    left_pane_pressed_at: Option<Instant>,
    right_pane_pressed_at: Option<Instant>,
    left_split_fired: bool,
    right_split_fired: bool,
    repeat_up: RepeatState,
    repeat_down: RepeatState,
    repeat_left: RepeatState,
    repeat_right: RepeatState,
    repeat_scroll_up: RepeatState,
    repeat_scroll_down: RepeatState,
}

#[derive(Default)]
struct RepeatState {
    next_emit_at: Option<Instant>,
}

impl RepeatState {
    fn tick(
        &mut self,
        active: bool,
        strength: f32,
        tx: &mpsc::UnboundedSender<TuiEvent>,
        event: TuiEvent,
        now: Instant,
    ) {
        if !active {
            self.next_emit_at = None;
            return;
        }
        match self.next_emit_at {
            None => {
                let _ = tx.send(event);
                self.next_emit_at = Some(now + GAMEPAD_REPEAT_DELAY);
            }
            Some(next_emit_at) if now >= next_emit_at => {
                let _ = tx.send(event);
                self.next_emit_at = Some(now + repeat_interval(strength));
            }
            Some(_) => {}
        }
    }
}

impl GamepadState {
    fn handle_event(&mut self, event: EventType, tx: &mpsc::UnboundedSender<TuiEvent>) {
        match event {
            EventType::ButtonPressed(button, _) => self.set_button(button, true, tx),
            EventType::ButtonReleased(button, _) => self.set_button(button, false, tx),
            EventType::AxisChanged(axis, value, _) => self.set_axis(axis, value),
            EventType::Disconnected | EventType::Dropped => *self = Self::default(),
            _ => {}
        }
    }

    fn set_button(&mut self, button: Button, pressed: bool, tx: &mpsc::UnboundedSender<TuiEvent>) {
        match button {
            Button::DPadUp => self.dpad_up = pressed,
            Button::DPadDown => self.dpad_down = pressed,
            Button::DPadLeft => self.dpad_left = pressed,
            Button::DPadRight => self.dpad_right = pressed,
            Button::LeftTrigger => Self::handle_hold_button(
                pressed,
                &mut self.left_shoulder_pressed_at,
                &mut self.left_new_tab_fired,
                tx,
                GamepadAction::ProjectTabPrevious,
            ),
            Button::RightTrigger => Self::handle_hold_button(
                pressed,
                &mut self.right_shoulder_pressed_at,
                &mut self.right_new_tab_fired,
                tx,
                GamepadAction::ProjectTabNext,
            ),
            Button::LeftTrigger2 => Self::handle_hold_button(
                pressed,
                &mut self.left_pane_pressed_at,
                &mut self.left_split_fired,
                tx,
                GamepadAction::SplitPaneFocusPrevious,
            ),
            Button::RightTrigger2 => Self::handle_hold_button(
                pressed,
                &mut self.right_pane_pressed_at,
                &mut self.right_split_fired,
                tx,
                GamepadAction::SplitPaneFocusNext,
            ),
            Button::Start | Button::Mode if pressed => {
                let _ = tx.send(TuiEvent::Gamepad(GamepadAction::OpenProjectNavigator));
            }
            Button::LeftThumb if pressed => {
                let _ = tx.send(TuiEvent::Gamepad(GamepadAction::ProjectWorkspacePrevious));
            }
            Button::RightThumb if pressed => {
                let _ = tx.send(TuiEvent::Gamepad(GamepadAction::ProjectWorkspaceNext));
            }
            Button::South if pressed => {
                let _ = tx.send(TuiEvent::Gamepad(GamepadAction::Confirm));
            }
            Button::North if pressed => {
                let _ = tx.send(TuiEvent::Gamepad(GamepadAction::Submit));
            }
            Button::East if pressed => {
                let _ = tx.send(TuiEvent::Gamepad(GamepadAction::Cancel));
            }
            Button::West if pressed => {
                let _ = tx.send(TuiEvent::Gamepad(GamepadAction::Context));
            }
            Button::Select => {
                let action = if pressed {
                    GamepadAction::PushToTalkStart
                } else {
                    GamepadAction::PushToTalkStop
                };
                let _ = tx.send(TuiEvent::Gamepad(action));
            }
            _ => {}
        }
    }

    fn handle_hold_button(
        pressed: bool,
        pressed_at: &mut Option<Instant>,
        hold_fired: &mut bool,
        tx: &mpsc::UnboundedSender<TuiEvent>,
        tap_action: GamepadAction,
    ) {
        if pressed {
            *pressed_at = Some(Instant::now());
            *hold_fired = false;
        } else {
            if pressed_at.take().is_some() && !*hold_fired {
                let _ = tx.send(TuiEvent::Gamepad(tap_action));
            }
            *hold_fired = false;
        }
    }

    fn set_axis(&mut self, axis: Axis, value: f32) {
        match axis {
            Axis::LeftStickX => self.left_x = value,
            Axis::LeftStickY => self.left_y = value,
            Axis::RightStickY => self.right_y = value,
            _ => {}
        }
    }

    fn emit_repeats(&mut self, tx: &mpsc::UnboundedSender<TuiEvent>) {
        let now = Instant::now();
        Self::emit_hold_action(
            &mut self.left_shoulder_pressed_at,
            &mut self.left_new_tab_fired,
            tx,
            GamepadAction::ProjectNewTabLeft,
            now,
        );
        Self::emit_hold_action(
            &mut self.right_shoulder_pressed_at,
            &mut self.right_new_tab_fired,
            tx,
            GamepadAction::ProjectNewTabRight,
            now,
        );
        Self::emit_hold_action(
            &mut self.left_pane_pressed_at,
            &mut self.left_split_fired,
            tx,
            GamepadAction::SplitPaneCreateHorizontal,
            now,
        );
        Self::emit_hold_action(
            &mut self.right_pane_pressed_at,
            &mut self.right_split_fired,
            tx,
            GamepadAction::SplitPaneCreateVertical,
            now,
        );

        let left = axis_strength(self.left_x, false, NAV_STICK_DEADZONE);
        let right = axis_strength(self.left_x, true, NAV_STICK_DEADZONE);
        let up = axis_strength(-self.left_y, true, NAV_STICK_DEADZONE);
        let down = axis_strength(self.left_y, true, NAV_STICK_DEADZONE);
        let scroll_up = axis_strength(-self.right_y, true, SCROLL_STICK_DEADZONE);
        let scroll_down = axis_strength(self.right_y, true, SCROLL_STICK_DEADZONE);
        self.repeat_up.tick(
            self.dpad_up || up > 0.0,
            up.max(if self.dpad_up { 1.0 } else { 0.0 }),
            tx,
            TuiEvent::Gamepad(GamepadAction::Up),
            now,
        );
        self.repeat_down.tick(
            self.dpad_down || down > 0.0,
            down.max(if self.dpad_down { 1.0 } else { 0.0 }),
            tx,
            TuiEvent::Gamepad(GamepadAction::Down),
            now,
        );
        self.repeat_left.tick(
            self.dpad_left || left > 0.0,
            left.max(if self.dpad_left { 1.0 } else { 0.0 }),
            tx,
            TuiEvent::Gamepad(GamepadAction::Left),
            now,
        );
        self.repeat_right.tick(
            self.dpad_right || right > 0.0,
            right.max(if self.dpad_right { 1.0 } else { 0.0 }),
            tx,
            TuiEvent::Gamepad(GamepadAction::Right),
            now,
        );
        self.repeat_scroll_up.tick(
            scroll_up > 0.0,
            scroll_up,
            tx,
            TuiEvent::Gamepad(GamepadAction::ScrollTranscriptLineUp),
            now,
        );
        self.repeat_scroll_down.tick(
            scroll_down > 0.0,
            scroll_down,
            tx,
            TuiEvent::Gamepad(GamepadAction::ScrollTranscriptLineDown),
            now,
        );
    }

    fn emit_hold_action(
        pressed_at: &mut Option<Instant>,
        hold_fired: &mut bool,
        tx: &mpsc::UnboundedSender<TuiEvent>,
        action: GamepadAction,
        now: Instant,
    ) {
        if !*hold_fired
            && pressed_at.is_some_and(|started| {
                now.saturating_duration_since(started) >= SHOULDER_HOLD_DURATION
            })
        {
            let _ = tx.send(TuiEvent::Gamepad(action));
            *hold_fired = true;
        }
    }
}

fn axis_strength(value: f32, positive_direction: bool, deadzone: f32) -> f32 {
    let component = if positive_direction {
        value.max(0.0)
    } else {
        (-value).max(0.0)
    };
    if component < deadzone { 0.0 } else { component }
}

fn repeat_interval(strength: f32) -> Duration {
    let strength = strength.clamp(0.0, 1.0);
    let span = GAMEPAD_MAX_REPEAT_INTERVAL - GAMEPAD_MIN_REPEAT_INTERVAL;
    GAMEPAD_MAX_REPEAT_INTERVAL - span.mul_f32(strength)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::Event;
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyModifiers;
    use pretty_assertions::assert_eq;
    use std::task::Context;
    use std::task::Poll;
    use std::time::Duration;
    use tokio::sync::broadcast;
    use tokio::sync::mpsc;
    use tokio::time::timeout;
    use tokio_stream::StreamExt;

    /// Simple fake event source for tests; feed events via the handle.
    struct FakeEventSource {
        rx: mpsc::UnboundedReceiver<EventResult>,
        tx: mpsc::UnboundedSender<EventResult>,
    }

    struct FakeEventSourceHandle {
        broker: Arc<EventBroker<FakeEventSource>>,
    }

    impl FakeEventSource {
        fn new() -> Self {
            let (tx, rx) = mpsc::unbounded_channel();
            Self { rx, tx }
        }
    }

    impl Default for FakeEventSource {
        fn default() -> Self {
            Self::new()
        }
    }

    impl FakeEventSourceHandle {
        fn new(broker: Arc<EventBroker<FakeEventSource>>) -> Self {
            Self { broker }
        }

        fn send(&self, event: EventResult) {
            let mut state = self
                .broker
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(source) = state.active_event_source_mut() else {
                return;
            };
            let _ = source.tx.send(event);
        }
    }

    impl EventSource for FakeEventSource {
        fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<EventResult>> {
            Pin::new(&mut self.get_mut().rx).poll_recv(cx)
        }
    }

    fn make_stream(
        broker: Arc<EventBroker<FakeEventSource>>,
        draw_rx: broadcast::Receiver<()>,
        terminal_focused: Arc<AtomicBool>,
    ) -> TuiEventStream<FakeEventSource> {
        TuiEventStream::new(
            broker,
            draw_rx,
            terminal_focused,
            #[cfg(unix)]
            crate::tui::job_control::SuspendContext::new(),
            #[cfg(unix)]
            Arc::new(AtomicBool::new(false)),
        )
    }

    type SetupState = (
        Arc<EventBroker<FakeEventSource>>,
        FakeEventSourceHandle,
        broadcast::Sender<()>,
        broadcast::Receiver<()>,
        Arc<AtomicBool>,
    );

    fn setup() -> SetupState {
        let source = FakeEventSource::new();
        let broker = Arc::new(EventBroker::new());
        *broker.state.lock().unwrap() = EventBrokerState::Running(source);
        let handle = FakeEventSourceHandle::new(broker.clone());

        let (draw_tx, draw_rx) = broadcast::channel(1);
        let terminal_focused = Arc::new(AtomicBool::new(true));
        (broker, handle, draw_tx, draw_rx, terminal_focused)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn key_event_skips_unmapped() {
        let (broker, handle, _draw_tx, draw_rx, terminal_focused) = setup();
        let mut stream = make_stream(broker, draw_rx, terminal_focused);

        handle.send(Ok(Event::FocusLost));
        handle.send(Ok(Event::Key(KeyEvent::new(
            KeyCode::Char('a'),
            KeyModifiers::NONE,
        ))));

        let next = stream.next().await.unwrap();
        match next {
            TuiEvent::Key(key) => {
                assert_eq!(key, KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
            }
            other => panic!("expected key event, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn draw_and_key_events_yield_both() {
        let (broker, handle, draw_tx, draw_rx, terminal_focused) = setup();
        let mut stream = make_stream(broker, draw_rx, terminal_focused);

        let expected_key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        let _ = draw_tx.send(());
        handle.send(Ok(Event::Key(expected_key)));

        let first = stream.next().await.unwrap();
        let second = stream.next().await.unwrap();

        let mut saw_draw = false;
        let mut saw_key = false;
        for event in [first, second] {
            match event {
                TuiEvent::Draw => {
                    saw_draw = true;
                }
                TuiEvent::Key(key) => {
                    assert_eq!(key, expected_key);
                    saw_key = true;
                }
                other => panic!("expected draw or key event, got {other:?}"),
            }
        }

        assert!(saw_draw && saw_key, "expected both draw and key events");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn lagged_draw_maps_to_draw() {
        let (broker, _handle, draw_tx, draw_rx, terminal_focused) = setup();
        let mut stream = make_stream(broker, draw_rx.resubscribe(), terminal_focused);

        // Fill channel to force Lagged on the receiver.
        let _ = draw_tx.send(());
        let _ = draw_tx.send(());

        let first = stream.next().await;
        assert!(matches!(first, Some(TuiEvent::Draw)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn resize_event_maps_to_resize() {
        let (broker, handle, _draw_tx, draw_rx, terminal_focused) = setup();
        let mut stream = make_stream(broker, draw_rx, terminal_focused);

        handle.send(Ok(Event::Resize(80, 24)));

        let next = stream.next().await;
        assert!(matches!(next, Some(TuiEvent::Resize)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn error_or_eof_ends_stream() {
        let (broker, handle, _draw_tx, draw_rx, terminal_focused) = setup();
        let mut stream = make_stream(broker, draw_rx, terminal_focused);

        handle.send(Err(std::io::Error::other("boom")));

        let next = stream.next().await;
        assert!(next.is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn resume_wakes_paused_stream() {
        let (broker, handle, _draw_tx, draw_rx, terminal_focused) = setup();
        let mut stream = make_stream(broker.clone(), draw_rx, terminal_focused);

        broker.pause_events();

        let task = tokio::spawn(async move { stream.next().await });
        tokio::task::yield_now().await;

        broker.resume_events();
        let expected_key = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE);
        handle.send(Ok(Event::Key(expected_key)));

        let event = timeout(Duration::from_millis(100), task)
            .await
            .expect("timed out waiting for resumed event")
            .expect("join failed");
        match event {
            Some(TuiEvent::Key(key)) => assert_eq!(key, expected_key),
            other => panic!("expected key event, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn resume_wakes_pending_stream() {
        let (broker, handle, _draw_tx, draw_rx, terminal_focused) = setup();
        let mut stream = make_stream(broker.clone(), draw_rx, terminal_focused);

        let task = tokio::spawn(async move { stream.next().await });
        tokio::task::yield_now().await;

        broker.pause_events();
        broker.resume_events();
        let expected_key = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE);
        handle.send(Ok(Event::Key(expected_key)));

        let event = timeout(Duration::from_millis(100), task)
            .await
            .expect("timed out waiting for resumed event")
            .expect("join failed");
        match event {
            Some(TuiEvent::Key(key)) => assert_eq!(key, expected_key),
            other => panic!("expected key event, got {other:?}"),
        }
    }

    #[test]
    fn shoulder_hold_opens_new_tab_once() {
        let mut state = GamepadState {
            left_shoulder_pressed_at: Some(Instant::now() - SHOULDER_HOLD_DURATION),
            ..Default::default()
        };
        let (tx, mut rx) = mpsc::unbounded_channel();

        state.emit_repeats(&tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(TuiEvent::Gamepad(GamepadAction::ProjectNewTabLeft))
        ));
        state.emit_repeats(&tx);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn trigger_hold_creates_split_once() {
        let mut state = GamepadState {
            right_pane_pressed_at: Some(Instant::now() - SHOULDER_HOLD_DURATION),
            ..Default::default()
        };
        let (tx, mut rx) = mpsc::unbounded_channel();

        state.emit_repeats(&tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(TuiEvent::Gamepad(GamepadAction::SplitPaneCreateVertical))
        ));
        state.emit_repeats(&tx);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn right_stick_scrolls_transcript_by_line() {
        let mut state = GamepadState {
            right_y: 1.0,
            ..Default::default()
        };
        let (tx, mut rx) = mpsc::unbounded_channel();

        state.emit_repeats(&tx);

        assert!(matches!(
            rx.try_recv(),
            Ok(TuiEvent::Gamepad(GamepadAction::ScrollTranscriptLineDown))
        ));
        assert!(rx.try_recv().is_err());
    }
}
