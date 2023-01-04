# Listenbrainz MPV
This is an MPV C-Plugin that scrobbles your music to ListenBrainz!

To compile, make a `.env` file with this content
```sh
USER_TOKEN="Token <your token>"
```
I made this for myself, and no one else. Don't expect any configurability or ease-of-use
improvements any time soon

By default, this plugin won't scrobble unless the track contains a MusicBrainz Recording MBID. To change this, compile with no default features
```sh
cargo build --release --no-default-features
```

You can also submit ListenBrainz feedback with this plugin using key bindings. For example, this is my `input.conf`
```
Ctrl+UP script-binding listenbrainz-love
Ctrl+DOWN script-binding listenbrainz-hate
Shift+Ctrl+DOWN script-binding listenbrainz-unrate
```

## Features

- *Now Playing* status on ListenBrainz
- Scrobbles based on ListenBrainz guidelines (at 4 minutes, or when half the song as elapsed)
- Allow for loving, hating, or removing feedback on a song
- *Complete* scrobbles with as much metadata as possible (including MBIDs)
  - This plugin assumes that you've used MusicBrainz Picard to tag your music, this plugin may break if this is untrue
- *utlra*lightweight
  - Because I didn't want to use an async runtime, I used `calloop` which relies on Linux's/BSD's polling systems. This means that this plugin is only compatible with Linux, but then again, C Plugins *only* work on Linux/BSD, so that doesn't really matter
- When offline, the plugin caches scrobbles and submits them *as soon* as your connection returns
  - This functionality is powered by `connman`'s dbus API, meaning that you must be using `connman` as your network manager to use this. Again, don't expect me to change this

## If you use this for nothing else, use this plugin as a template.

I made this plugin with the goal of being intensly lightweight with almost zero runtime overhead¸ my goals for this project were

- No spawning threads (mpv already spawns a lot)
  - By proxy, no async runtime
- No polling anything, even mpv's events
- Ideally, as little allocation as possible

This was extremely hard to acheive and it took me a *long* time to figure out how to do all that. So if noting else, use this crate as a template for *your own* mpv plugin!