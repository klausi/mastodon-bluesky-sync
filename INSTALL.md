# Installation of Mastodon Bluesky Sync

There are 2 options how to run mastodon-bluesky-sync:

1. Compiling yourself (takes a bit of time with the Rust compiler)
2. Docker

## Option 1: Compiling with cargo

For converting Bluesky video streams this program needs the `ffmpeg` executable. Install it for example on Debian/Ubuntu:
```sh
sudo apt install ffmpeg
```

Compile with Rust:

```
curl https://sh.rustup.rs -sSf | sh
source ~/.cargo/env
```
When running the program the first time a registration step will setup API access to Mastodon and Bluesky. Follow the text instructions to enter credentials.
```
git clone https://github.com/klausi/mastodon-bluesky-sync.git
cd mastodon-bluesky-sync
cargo run --release
```

Use the `cargo run --release --` command or `target/release/mastodon-bluesky-sync` as a replacement for `./mastodon-bluesky-sync` in the examples in the README.

Configuration and cache files will be created in the directory where the program was executed.

## Option 2: Installing with Docker

You need to have Docker installed on your system, then you can use the [published Docker image](https://hub.docker.com/r/klausi/mastodon-bluesky-sync).

The following commands create a directory where the settings file and cache files will be stored. Then we use a Docker volume from that directory to store them persistently.

```
mkdir mastodon-bluesky-sync
cd mastodon-bluesky-sync
docker run -it --rm -v "$(pwd)":/data klausi/mastodon-bluesky-sync
```

Follow the text instructions to enter API keys.

Use that Docker command as a replacement for `./mastodon-bluesky-sync` in the examples in this README.
