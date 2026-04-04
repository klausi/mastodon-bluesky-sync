Enable the repository-managed Git hooks with:

```sh
git config core.hooksPath .githooks
```

The `reference-transaction` hook blocks creation or update of Git tags unless the tag name matches the package version from `Cargo.toml` in `vX.Y.Z` format.
