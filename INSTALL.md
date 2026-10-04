# Installation of Mastodon Bluesky Sync

There are 3 options how to run mastodon-bluesky-sync:

1. Downloading a pre-built binary
2. Compiling yourself (takes a bit of time with the Rust compiler)
3. Docker

## Option 1: Downloading a pre-built binary

Download the archive for your operating system and CPU architecture from the [GitHub release pages](https://github.com/klausi/mastodon-bluesky-sync/releases). Pre-built binaries are available for Linux (x86_64), macOS (Apple Silicon and Intel), and Windows (x86_64). Extract the archive into a directory of your choice.

For converting Bluesky video streams this program needs the `ffmpeg` executable. Install it and make sure it is available in your `PATH`. For example, on Debian/Ubuntu:

```sh
sudo apt install ffmpeg
```

Open a terminal in the directory containing the extracted binary and run:

```sh
./mastodon-bluesky-sync
```

On Windows, use `./mastodon-bluesky-sync.exe` in PowerShell.

When running the program for the first time, follow the text instructions to set up API access to Mastodon and Bluesky. Configuration and cache files will be created in the directory where the program was executed. See the README for further usage examples.

## Option 2: Compiling with cargo

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

## Option 3: Installing with Docker

You need to have Docker installed on your system, then you can use the [published Docker image](https://hub.docker.com/r/klausi/mastodon-bluesky-sync).

The following commands create a directory where the settings file and cache files will be stored. Then we use a Docker volume from that directory to store them persistently.

```
mkdir mastodon-bluesky-sync
cd mastodon-bluesky-sync
docker run -it --rm -v "$(pwd)":/data klausi/mastodon-bluesky-sync
```

Follow the text instructions to enter API keys.

Use that Docker command as a replacement for `./mastodon-bluesky-sync` in the examples in this README.
