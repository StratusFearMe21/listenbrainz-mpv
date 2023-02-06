# Listenbrainz MPV
This is an MPV C-Plugin that scrobbles your music to ListenBrainz!


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

## Configuration

You must configure this plugin via the `script-opts` option in `mpv.conf`, this is an example
```
script-opts=listenbrainz-user-token={YOUR_USER_TOKEN},listenbrainz-cache-path=.cache
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
  - You must add `--features connman` to your compile command to use this feature, and you must be using `connman` as your network manager.
  - On Android, this does not apply

## Android

This plugin is compatible with the Android version of MPV via [my tutorial](https://www.reddit.com/r/mpv/comments/107oasp/c_plugins_in_mpv_on_android).

This plugin requires this compilation command

```sh
CC=$NDK_TOOLCHAIN/bin/armv7a-linux-androideabi29-clang AR=$NDK_TOOLCHAIN/bin/llvm-ar cargo +nightly build --release -Zbuild-std --target="armv7-linux-androideabi"
```

For 64-bit build

```sh
CC=$NDK_TOOLCHAIN/bin/aarch64-linux-android29-clang AR=$NDK_TOOLCHAIN/bin/llvm-ar cargo +nightly build --release -Zbuild-std --target="aarch64-linux-android"
```

If the plugin crashes `mpv-android`, try setting `listenbrainz-cache-path` to the path to your SD card.

## If you use this for nothing else, use this plugin as a template.

I made this plugin with the goal of being intensly lightweight with almost zero runtime overhead¸ my goals for this project were

- No spawning threads (mpv already spawns a lot)
  - By proxy, no async runtime
- No polling anything, even mpv's events
- Ideally, as little allocation as possible

This was extremely hard to acheive and it took me a *long* time to figure out how to do all that. So if noting else, use this crate as a template for *your own* mpv plugin!

