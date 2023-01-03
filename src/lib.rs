use std::{
    io::BufWriter,
    mem::ManuallyDrop,
    num::NonZeroU64,
    time::{Duration, SystemTime},
};

use calloop::{
    channel::{Channel, Sender},
    timer::Timer,
    EventLoop,
};
use dbus::message::MatchRule;
use libmpv::{events::Event, Mpv, MpvStr};
use libmpv_sys::mpv_handle;
use serde::Serialize;

mod connman;

macro_rules! cache_path {
    () => {
        dirs::home_dir()
            .unwrap()
            .join(dirs::cache_dir().unwrap())
            .join("listenbrainz")
    };
}

#[derive(Serialize, Default, Debug)]
struct ListenbrainzData {
    payload: Payload,
    scrobble: bool,
    online: bool,
}

#[derive(Serialize, Debug)]
struct ListenbrainzSingleListen<'a> {
    listen_type: &'static str,
    payload: [&'a Payload; 1],
}

#[derive(Serialize, Default, Debug)]
struct Payload {
    #[serde(skip_serializing_if = "Option::is_none")]
    listened_at: Option<NonZeroU64>,
    track_metadata: TrackMetadata,
}

#[derive(Serialize, Default, Debug)]
struct TrackMetadata {
    additional_info: AdditionalInfo,
    artist_name: String,
    track_name: String,
    release_name: String,
}

#[derive(Serialize, Debug)]
struct AdditionalInfo {
    media_player: &'static str,
    submission_client: &'static str,
    submission_client_version: &'static str,
    #[serde(skip_serializing_if = "String::is_empty")]
    release_mbid: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    artist_mbids: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    recording_mbid: String,
    duration_ms: u64,
}

impl Default for AdditionalInfo {
    fn default() -> Self {
        Self {
            media_player: "mpv",
            submission_client: "mpv ListenBrainz Rust",
            submission_client_version: env!("CARGO_PKG_VERSION"),
            release_mbid: String::new(),
            artist_mbids: Vec::new(),
            recording_mbid: String::new(),
            duration_ms: 0,
        }
    }
}

fn scrobble(listen_type: &'static str, payload: &Payload, online: bool) {
    let send = ListenbrainzSingleListen {
        listen_type,
        payload: [payload],
    };
    // println!("{}", serde_json::to_string_pretty(&send).unwrap());
    if online {
        let status = ureq::post("https://api.listenbrainz.org/1/submit-listens")
            .set("Authorization", USER_TOKEN)
            .send_json(send)
            .unwrap()
            .status();
        if status != 200 {
            panic!("Scrobble failed: {}", status);
        }
    } else {
        let cache_path = cache_path!();
        if !cache_path.exists() {
            std::fs::create_dir(cache_path).unwrap();
        }
        serde_json::to_writer(
            BufWriter::new(
                std::fs::File::create(format!("{}.json", payload.listened_at.unwrap())).unwrap(),
            ),
            &payload,
        )
        .unwrap();
    }
}

fn import_cache() {
    let cache_path = cache_path!();
    if cache_path.exists() {
        println!("Scrobbling unscrobbled music");
        let mut request = br#"{"listen_type":"import","payload":["#.to_vec();
        for i in std::fs::read_dir("cache_path").unwrap() {
            std::io::copy(
                &mut std::fs::File::open(i.unwrap().path()).unwrap(),
                &mut request,
            )
            .unwrap();
            request.push(b',');
        }
        request.pop();
        request.extend_from_slice(b"]}");
        std::fs::remove_dir_all(cache_path).unwrap();
        let status = ureq::post("https://api.listenbrainz.org/1/submit-listens")
            .set("Authorization", USER_TOKEN)
            .set("Content-Type", "json")
            .send_bytes(&request)
            .unwrap()
            .status();
        if status != 200 {
            panic!("Importing scrobbles failed: {}", status);
        }
    }
}

use dotenvy_macro::dotenv;
const USER_TOKEN: &str = dotenv!("USER_TOKEN");

#[no_mangle]
pub extern "C" fn mpv_open_cplugin(ctx: *mut mpv_handle) -> i8 {
    let mut mpv = ManuallyDrop::new(Mpv::new_with_context(ctx).unwrap());
    let mut event_loop = EventLoop::<ListenbrainzData>::try_new().unwrap();
    let handle = event_loop.handle();
    let timer = Timer::from_duration(Duration::from_secs(31_536_000));
    let mut timer = handle
        .insert_source(timer, |_event, _metadata, data| {
            println!("How? How has MPV not loaded your file for an entire year?");
            if data.scrobble {
                data.payload.listened_at = NonZeroU64::new(
                    SystemTime::now()
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                );
                scrobble("single", &data.payload, data.online);
            }
            calloop::timer::TimeoutAction::Drop
        })
        .unwrap();
    let (tx, rx): (Sender<()>, Channel<()>) = calloop::channel::channel();
    mpv.event_context_mut()
        .set_wakeup_callback(move || tx.send(()).unwrap());
    let signal = event_loop.get_signal();

    let rx_handle = event_loop.handle();

    handle
        .insert_source(rx, move |_event, _metadata, data| loop {
            match mpv.event_context_mut().wait_event(0.0) {
                Some(Ok(Event::Shutdown)) => signal.stop(),
                Some(Ok(Event::PlaybackRestart)) => {
                    let audio_pts: Result<i64, libmpv::Error> = mpv.get_property("audio-pts");
                    if audio_pts.is_err() || audio_pts.unwrap() < 1 {
                        *data = ListenbrainzData::default();
                        rx_handle.remove(timer);
                        let duration = mpv.get_property::<i64>("duration").unwrap() as u64;
                        data.payload.track_metadata.additional_info.duration_ms = duration * 1000;
                        timer = rx_handle
                            .insert_source(
                                Timer::from_duration(Duration::from_secs(std::cmp::min(
                                    240,
                                    duration / 2,
                                ))),
                                |_event, _metadata, data| {
                                    if data.scrobble {
                                        data.payload.listened_at = NonZeroU64::new(
                                            SystemTime::now()
                                                .duration_since(SystemTime::UNIX_EPOCH)
                                                .unwrap()
                                                .as_secs(),
                                        );
                                        scrobble("single", &data.payload, data.online);
                                    }
                                    calloop::timer::TimeoutAction::Drop
                                },
                            )
                            .unwrap();

                        for i in mpv
                            .get_property::<libmpv::MpvNode>("metadata")
                            .unwrap()
                            .to_map()
                            .unwrap()
                        {
                            match i.0 {
                                "MUSICBRAINZ_ALBUMID" | "MusicBrainz Album Id" => {
                                    data.payload.track_metadata.additional_info.release_mbid =
                                        i.1.to_str().unwrap().to_string()
                                }
                                "MUSICBRAINZ_ARTISTID" | "MusicBrainz Artist Id" => {
                                    data.payload.track_metadata.additional_info.artist_mbids =
                                        i.1.to_str()
                                            .unwrap()
                                            .split(";")
                                            .map(|f| f.trim().to_string())
                                            .collect();
                                }
                                "MUSICBRAINZ_TRACKID" => {
                                    data.payload.track_metadata.additional_info.recording_mbid =
                                        i.1.to_str().unwrap().to_string();
                                }
                                "ARTIST" | "artist" => {
                                    data.payload.track_metadata.artist_name =
                                        i.1.to_str().unwrap().to_string();
                                }
                                "TITLE" | "title" => {
                                    data.payload.track_metadata.track_name =
                                        i.1.to_str().unwrap().to_string();
                                }
                                "ALBUM" | "album" => {
                                    data.payload.track_metadata.release_name =
                                        i.1.to_str().unwrap().to_string();
                                }
                                _ => {}
                            }
                        }

                        data.scrobble = (*mpv.get_property::<MpvStr>("filename").unwrap()
                            != data.payload.track_metadata.track_name)
                            && !data.payload.track_metadata.artist_name.is_empty()
                            && !data.payload.track_metadata.track_name.is_empty()
                            && !data.payload.track_metadata.release_name.is_empty();

                        if data.scrobble && data.online {
                            data.payload.listened_at = None;
                            scrobble("playing_now", &data.payload, data.online);
                        }
                    }
                }
                None => break,
                _ => {}
            }
        })
        .unwrap();

    let mut data = ListenbrainzData::default();

    data.online = {
        let (system_connection, _sender): (calloop_dbus::DBusSource<()>, _) =
            calloop_dbus::DBusSource::new_system().unwrap();
        let connman_proxy =
            system_connection.with_proxy("net.connman", "/", Duration::from_secs(5));
        let properties = connman_proxy
            .method_call("net.connman.Manager", "GetProperties", ())
            .and_then(|r: (dbus::arg::PropMap,)| Ok(r.0))
            .unwrap();
        system_connection
            .add_match::<connman::NetConnmanManagerPropertyChanged, _>(
                MatchRule::new_signal("net.connman.Manager", "PropertyChanged"),
                |_, _, _| true,
            )
            .unwrap();

        handle
            .insert_source(system_connection, |event, _metadata, data| {
                if let Some(member) = event.member() {
                    if &*member == "PropertyChanged" {
                        let property: connman::NetConnmanManagerPropertyChanged =
                            event.read_all().unwrap();
                        if property.name == "State" {
                            let val = property.value.0.as_str().unwrap();
                            data.online = val == "ready" || val == "online";
                            if data.online {
                                import_cache();
                            }
                        }
                    }
                }
                None
            })
            .unwrap();
        let state = properties
            .get("State")
            .unwrap()
            .0
            .as_str()
            .unwrap_or_default();
        state == "ready" || state == "online"
    };
    drop(handle);

    if data.online {
        import_cache();
    }

    event_loop.run(None, &mut data, |_| {}).unwrap();
    return 0;
}
