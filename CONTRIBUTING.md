# Contributing

ChatPlus Desktop for macOS is maintained by turbogizzmo. Contributions to bugs, provider
integration, translations, accessibility, macOS testing and documentation are welcome.

Start with [local development](docs/DEVELOPMENT.md), [architecture](docs/ARCHITECTURE.md)
and [testing](docs/TESTING.md). The lightweight Tauri 2, Rust and vanilla TypeScript
architecture and clearly unofficial branding should remain intact.

- Use focused changes and small logical commits matching recent history. Reproduce
  bugs and add regression coverage; run tests after each meaningful subsystem change.
- Preserve useful existing comments, including the original author's `Swiph3l:` notes.
  Explain new non-obvious constraints with a short why-comment; avoid restating obvious code.
- Add provider-specific semantic adapters and fixtures; inspect the actual provider
  DOM. Keep shared lifecycle/state logic outside adapters and remote views outside
  native capabilities. Do not add undocumented private APIs or automatic media grants.
- Notification diagnostics must omit message content, credentials/tokens, tags and
  private URLs. Never commit test accounts, profiles, keys, logs or private screenshots.
- Before review, run `git diff --check`, inspect staged/untracked files and record
  automatic results and native macOS checks actually performed. Keep release notes
  user-readable; investigations belong in docs.

CI validates macOS only and uploads an unsigned `.dmg` artifact. Native runtime
acceptance is separate from a successful build. There is no automated release workflow.

By submitting a contribution, you agree that accepted work is distributed under
[GNU GPLv3 (GPL-3.0-only)](LICENSE). You retain your copyright; there is no copyright
transfer or CLA. Contribute only work you have the right to submit and preserve
third-party licenses/notices. See [licensing notes](docs/LICENSING.md) and
[security reporting](SECURITY.md).

Useful issue-label suggestions for maintainers: bug, good first issue, help wanted,
notifications, provider, windows, translations and documentation. These are
recommendations; contribution changes do not create repository labels/settings.
