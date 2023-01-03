# Listenbrainz MPV
This is an MPV C-Plugin that scrobbles your music to ListenBrainz!

To compile, make a `.env` file with this content
```sh
USER_TOKEN="Token <your token>"
```
I made this for myself, and no one else. Don't expect any configurability or ease-of-use
improvements any time soon

## Features
- *Now Playing* status on ListenBrainz
- Scrobbles based on ListenBrainz guidelines (at 4 minutes, or when half the song as elapsed)
- *utlra*lightweight
  - Almost to a fault, because I didn't want to use an *async* runtime I used `calloop` which relies on Linux's/BSD's polling systems. This means that this is only compatible with Linux, but then again, C Plugins *only* work on Linux/BSD, so that doesn't really matter
- When offline, it caches scrobbles and submits them *as soon* as your connection returns
  - This functionality is powered by `connman`'s dbus API, meaning that you must be using `connman` as your network manager to use this. Again, don't expect me to change this