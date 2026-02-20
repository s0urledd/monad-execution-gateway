# Contributing

Thanks for your interest in the Monad Execution Events Gateway! This project is community-maintained and contributions are welcome.

## Getting Started

1. Fork the repository and create a feature branch from `main`.
2. Follow the [Deployment Guide](docs/DEPLOYMENT.md) to set up a local build.
3. Make your changes and verify they compile (`cargo build --release` in the `gateway/` directory).
4. Open a pull request against `main`.

## Project Structure

```
gateway/          Rust gateway server (event listener, WebSocket, REST)
sdk/typescript/   TypeScript client SDK
docs/             API, events, and deployment documentation
examples/         Usage examples
```

## Commit Convention

Use [Conventional Commits](https://www.conventionalcommits.org/):

```
feat: add new WebSocket channel for lifecycle events
fix: prevent double-send on cursor resume edge case
docs: update API reference for backpressure section
```

**Types:** `feat`, `fix`, `docs`, `refactor`, `perf`, `test`, `chore`

When merging, squash commits into a single well-described commit.

## Pull Request Guidelines

- Keep PRs focused — one feature or fix per PR.
- Include a clear description of **what** changed and **why**.
- Add or update tests where applicable.
- Make sure `cargo fmt --check` and `cargo clippy` pass.

## Code Style

- **Rust:** Follow standard `rustfmt` formatting. Use `cargo fmt` before committing.
- **TypeScript:** Follow the existing style in `sdk/typescript/`.

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](LICENSE).
