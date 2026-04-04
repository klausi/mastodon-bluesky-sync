
## Release tag check

This repository includes a Git `reference-transaction` hook that rejects creating or updating tags unless the tag name matches the version in `Cargo.toml` using the `vX.Y.Z` format.

Enable the hook once in your clone:

```sh
git config core.hooksPath .githooks
```
