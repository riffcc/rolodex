<p align="center"><code>npm i -g @openai/codex</code><br />or <code>brew install --cask codex</code></p>
<p align="center"><strong>Codex CLI</strong> is a coding agent from OpenAI that runs locally on your computer.
<p align="center">
  <img src="https://github.com/openai/codex/blob/main/.github/codex-cli-splash.png" alt="Codex CLI splash" width="80%" />
</p>
</br>
If you want Codex in your code editor (VS Code, Cursor, Windsurf), <a href="https://developers.openai.com/codex/ide">install in your IDE.</a>
</br>If you want the desktop app experience, run <code>codex app</code> or visit <a href="https://chatgpt.com/codex?app-landing-page=true">the Codex App page</a>.
</br>If you are looking for the <em>cloud-based agent</em> from OpenAI, <strong>Codex Web</strong>, go to <a href="https://chatgpt.com/codex">chatgpt.com/codex</a>.</p>

---

## Rolodex

**This fork is Rolodex** -- the [Riff Labs](https://riff.cc) distribution of Codex CLI.

### Why Rolodex?

Codex is an excellent foundation, but it's built around a single provider (OpenAI) and a single interaction model (text-in, text-out). Rolodex takes the Codex core and pushes it in directions that matter for real-world, multi-provider agent work:

- **Voice-first interaction.** Rolodex integrates the [Handy.computer](https://handy.computer) voice-to-text engine, making it possible to drive your coding agent by speaking naturally instead of typing. This isn't a wrapper around a generic STT API -- it's purpose-built for developer workflows where you're dictating code intent, describing bugs, or narrating architectural decisions while your hands are busy.
- **Broader model compatibility.** Codex already supports multiple providers out of the box. Rolodex extends that work with compatibility shims for models that don't fully implement the OpenAI API (such as some Chinese-market models), so more endpoints just work without manual configuration.
- **Opinionated packaging.** Rolodex ships as a single binary (`rolodex`) with `.deb` packages for Debian/Ubuntu, prerelease builds on every push to main, and stable releases cut from tags. No npm required if you don't want it.
- **Community-driven.** Rolodex is an open fork that welcomes contributions from everyone. We build on the excellent work OpenAI does upstream and add what our users need.

### Acknowledgements

Voice-to-text engine integrations in Rolodex are powered by **[Handy.computer](https://handy.computer)**. Their work on low-latency, developer-aware speech recognition makes voice-first coding actually practical -- not a novelty, but a genuine productivity multiplier. We're grateful for their partnership and their commitment to building tools that meet developers where they are.

---

## Quickstart

### Installing and running Codex CLI

Install globally with your preferred package manager:

```shell
# Install using npm
npm install -g @openai/codex
```

```shell
# Install using Homebrew
brew install --cask codex
```

Then simply run `codex` to get started.

<details>
<summary>You can also go to the <a href="https://github.com/openai/codex/releases/latest">latest GitHub Release</a> and download the appropriate binary for your platform.</summary>

Each GitHub Release contains many executables, but in practice, you likely want one of these:

- macOS
  - Apple Silicon/arm64: `codex-aarch64-apple-darwin.tar.gz`
  - x86_64 (older Mac hardware): `codex-x86_64-apple-darwin.tar.gz`
- Linux
  - x86_64: `codex-x86_64-unknown-linux-musl.tar.gz`
  - arm64: `codex-aarch64-unknown-linux-musl.tar.gz`

Each archive contains a single entry with the platform baked into the name (e.g., `codex-x86_64-unknown-linux-musl`), so you likely want to rename it to `codex` after extracting it.

</details>

### Using Codex with your ChatGPT plan

Run `codex` and select **Sign in with ChatGPT**. We recommend signing into your ChatGPT account to use Codex as part of your Plus, Pro, Team, Edu, or Enterprise plan. [Learn more about what's included in your ChatGPT plan](https://help.openai.com/en/articles/11369540-codex-in-chatgpt).

You can also use Codex with an API key, but this requires [additional setup](https://developers.openai.com/codex/auth#sign-in-with-an-api-key).

## Docs

- [**Codex Documentation**](https://developers.openai.com/codex)
- [**Contributing**](./docs/contributing.md)
- [**Installing & building**](./docs/install.md)
- [**Open source fund**](./docs/open-source-fund.md)

This repository is licensed under the [Apache-2.0 License](LICENSE).
