# Mastodon Bluesky Sync

[![Automated tests](https://github.com/klausi/mastodon-bluesky-sync/workflows/Testing/badge.svg)](https://github.com/klausi/mastodon-bluesky-sync/actions)

This tool synchronizes posts from [Mastodon](https://joinmastodon.org/) to [Bluesky](https://bsky.app) and back. It does not matter where you post your stuff - it will get synchronized to the other!

## Synchronization Features

- Your status update on Bluesky will be posted automatically to Mastodon
- Your Repost on Bluesky will automatically be posted to Mastodon with a "♻️ username:" prefix
- Your status update on Mastodon will be posted automatically to Bluesky
- Your boost on Mastodon will be posted automatically to Bluesky with a "♻️ username:" prefix

## Old data deletion feature for better privacy
- Optionally a configuration option can be set to delete posts from your Bluesky account that are older than 90 days.
- Optionally a configuration option can be set to delete favorites (likes) from your Bluesky account that are older than 90 days.
- Optionally a configuration option can be set to delete favorites from your Mastodon account that are older than 90 days.

## Installation and execution

See [INSTALL.md](INSTALL.md).

## Configuration

All configuration options are created in a `mastodon-bluesky-sync.toml` file in the directory where you executed the program.

Example:

```toml
[mastodon]
base_url = "https://mastodon.social"
client_id = "XXXXXXXXXXXXXXXXX"
client_secret = "XXXXXXXXXXXXXXXXXXX"
access_token = "XXXXXXXXXXXXXXXXXXXXXXXXX"
refresh_token = "none"
sync_reblogs = true
sync_hashtag = ""
# Delete older Mastodon favorites that are older than 90 days.
delete_older_favs = true

[bluesky]
email = "klausi@example.com"
app_password = "XXXXXXXXXXXXXXXXXXXXXXX"
sync_reposts = true
sync_hashtag = ""
# Delete Bluesky posts that are older than 90 days.
delete_older_posts = true
# Delete older Bluesky favorites (likes) that are older than 90 days.
delete_older_favs = true
```

## Preview what's going to be synced

You can preview what's going to be synced using the `--dry-run` option:

    ./mastodon-bluesky-sync --dry-run

This is running a sync without actually posting anything.

## Skip existing posts and only sync new posts

If you already have posts in one or both of your accounts and you want to exclude them from being synced you can use `--skip-existing-posts`. This is going to mark all posts as synced without actually posting them.

    ./mastodon-bluesky-sync --skip-existing-posts

Note that combining `--skip-existing-posts --dry-run` will not do anything. You have to run `--skip-existing-posts` alone to mark all posts as synchronized in the post cache.

## Periodic execution

Every run of the program only synchronizes the accounts once. Use Cron to run it periodically, recommended every 10 minutes as in this example:

```
*/10 * * * *   cd /home/klausi/workspace/mastodon-bluesky-sync && ./mastodon-bluesky-sync
```

## Roadmap

Todo list for the future, not implemented yet:
- Your own threads (your replies to your own posts) will be synced both ways
- Build portable binaries on Github without OpenSSL dependencies
- Add open graph link preview when posting to Bluesky
- Parallel execution of fetching and syncing requests at the same time

## Acknowledgements

Thanks to [Yoshihiro Sugi (sugyan)](https://github.com/sugyan) for his continuous support when implementing Bluesky API access with [Atrium](https://github.com/sugyan/atrium).
