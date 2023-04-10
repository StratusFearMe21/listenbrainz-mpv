use std::{
    io::BufWriter,
    mem::ManuallyDrop,
    num::NonZeroU64,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

use calloop::{
    channel::{Channel, Sender},
    timer::Timer,
    EventLoop,
};
#[cfg(feature = "connman")]
use dbus::message::MatchRule;
use id3::{Content, Tag};
use libmpv::{
    events::{Event, PropertyData},
    Mpv, MpvStr,
};
use libmpv_sys::mpv_handle;
use serde::Serialize;

#[cfg(feature = "connman")]
mod connman;

#[derive(Debug)]
struct ListenbrainzData {
    payload: Payload,
    scrobble: bool,
    token: String,
    cache_path: PathBuf,
    online: bool,
    scrobble_deadline: Instant,
    pause_instant: Instant,
}

impl Default for ListenbrainzData {
    fn default() -> Self {
        Self {
            payload: Payload::default(),
            scrobble: false,
            token: String::new(),
            cache_path: {
                #[cfg(target_os = "linux")]
                {
                    dirs::cache_dir().unwrap().join("listenbrainz")
                }
                #[cfg(target_os = "android")]
                {
                    Path::new("/storage/emulated/0").join("listenbrainz")
                }
            },
            online: false,
            scrobble_deadline: Instant::now(),
            pause_instant: Instant::now(),
        }
    }
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

#[derive(Serialize, Default, Debug)]
struct LoveHate<'a> {
    recording_mbid: &'a str,
    score: i32,
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

fn scrobble(
    listen_type: &'static str,
    payload: &Payload,
    online: bool,
    token: &str,
    cache_path: &Path,
) {
    let send = ListenbrainzSingleListen {
        listen_type,
        payload: [payload],
    };
    #[cfg(debug_assertions)]
    eprintln!("{}", serde_json::to_string_pretty(&send).unwrap());
    if online {
        let status = ureq::post("https://api.listenbrainz.org/1/submit-listens")
            .set("Authorization", token)
            .send_json(send);
        if status.is_ok() {
            import_cache(token, cache_path);
            return;
        }
    }
    if let Some(listened_at) = payload.listened_at {
        serde_json::to_writer(
            BufWriter::new(
                std::fs::File::create(cache_path.join(format!("{}.json", listened_at))).unwrap(),
            ),
            &payload,
        )
        .unwrap();
    }
}

fn import_cache(token: &str, cache_path: &Path) {
    let mut read_dir = cache_path.read_dir().unwrap();
    let is_occupied = read_dir.next().is_some();
    let is_one_file = read_dir.next().is_none();
    if cache_path.exists() && is_occupied {
        let mut request = if is_one_file {
            br#"{"listen_type":"single","payload":["#.to_vec()
        } else {
            br#"{"listen_type":"import","payload":["#.to_vec()
        };
        for i in std::fs::read_dir(&cache_path).unwrap() {
            let path = i.unwrap().path();
            std::io::copy(
                &mut std::fs::File::open(path.as_path()).unwrap(),
                &mut request,
            )
            .unwrap();
            request.push(b',');
        }
        request.pop();
        request.extend_from_slice(b"]}");
        #[cfg(debug_assertions)]
        eprintln!("{}", unsafe { std::str::from_utf8_unchecked(&request) });
        let status = ureq::post("https://api.listenbrainz.org/1/submit-listens")
            .set("Authorization", token)
            .set("Content-Type", "json")
            .send_bytes(&request);
        if status.is_err() {
            eprintln!("Error importing {:?}", status);
            return;
        }
        std::fs::read_dir(cache_path)
            .unwrap()
            .try_for_each(|i| std::fs::remove_file(i?.path()))
            .unwrap();
    }
}

fn read_recording_id(filename: &str, data: &mut ListenbrainzData) -> Result<(), ()> {
    let Ok(tag) = Tag::read_from_path(filename) else {
        return Err(());
    };

    for f in tag.frames() {
        if f.id() == "UFID" {
            let Content::Unknown(ref u) = f.content() else {
                continue;
            };

            let Some(delimeter_pos) = memchr::memchr(0, &u.data) else {
                continue;
            };

            if &u.data[..delimeter_pos] != b"http://musicbrainz.org" {
                continue;
            }

            data.payload.track_metadata.additional_info.recording_mbid =
                if let Ok(s) = std::str::from_utf8(&u.data[delimeter_pos + 1..]) {
                    s.to_string()
                } else {
                    continue;
                };

            return Ok(());
        }
    }

    Err(())
}

macro_rules! scrobble_duration {
    ($duration:expr,$speed:expr) => {
        if $duration <= 40.0 {
            $duration - 1.0
        } else {
            f64::min(240.0, $duration / 2.0)
        } / $speed
    };
}

#[no_mangle]
pub extern "C" fn mpv_open_cplugin(ctx: *mut mpv_handle) -> i8 {
    let mut mpv = ManuallyDrop::new(Mpv::new_with_context(ctx).unwrap());
    mpv.event_context()
        .observe_property("pause", libmpv::Format::Flag, 0)
        .unwrap();
    mpv.event_context()
        .observe_property("speed", libmpv::Format::Double, 0)
        .unwrap();
    let mut event_loop = EventLoop::<ListenbrainzData>::try_new().unwrap();
    let handle = event_loop.handle();
    let timer = Timer::from_duration(Duration::from_secs(31_536_000));
    let mut timer = handle
        .insert_source(timer, |_event, _metadata, _data| {
            panic!(
                "Something has gone horibbly wrong, somehow, mpv has been loading for an entire \
                 year!"
            );
        })
        .unwrap();
    let (tx, rx): (Sender<()>, Channel<()>) = calloop::channel::channel();
    mpv.event_context_mut()
        .set_wakeup_callback(move || tx.send(()).unwrap());
    let signal = event_loop.get_signal();

    let rx_handle = event_loop.handle();

    fn timer_event(
        _event: Instant,
        _metadata: &mut (),
        data: &mut ListenbrainzData,
    ) -> calloop::timer::TimeoutAction {
        if data.scrobble {
            data.payload.listened_at = NonZeroU64::new(
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            );
            scrobble(
                "single",
                &data.payload,
                data.online,
                &data.token,
                &data.cache_path,
            );
        }
        data.scrobble = false;
        calloop::timer::TimeoutAction::Drop
    }

    let mut data = ListenbrainzData::default();

    for i in mpv
        .get_property::<libmpv::MpvNode>("script-opts")
        .unwrap()
        .to_map()
        .unwrap()
    {
        match i.0 {
            "listenbrainz-user-token" => data.token = format!("Token {}", i.1.to_str().unwrap()),
            "listenbrainz-cache-path" => {
                #[cfg(target_os = "linux")]
                {
                    data.cache_path = dirs::cache_dir()
                        .unwrap()
                        .join(i.1.to_str().unwrap())
                        .join("listenbrainz");
                }
                #[cfg(target_os = "android")]
                {
                    data.cache_path = Path::new(i.1.to_str().unwrap()).join("listenbrainz");
                }
            }
            _ => {}
        }
    }

    if let Ok(config_dir) = mpv.get_property::<MpvStr>("config-dir") {
        if !config_dir.is_empty() {
            data.cache_path = Path::new(&*config_dir).join("listenbrainz");
        }
    }

    if !data.cache_path.exists() {
        std::fs::create_dir(&data.cache_path).unwrap();
    }

    handle
        .insert_source(rx, move |_event, _metadata, data| loop {
            match mpv.event_context_mut().wait_event(0.0) {
                Some(Ok(Event::Shutdown)) => signal.stop(),
                Some(Ok(Event::ClientMessage(m))) => {
                    if m[0] == "key-binding" {
                        let score = match m[1] {
                            "listenbrainz-love" => 1,
                            "listenbrainz-hate" => -1,
                            "listenbrainz-unrate" => 0,
                            _ => continue,
                        };

                        if data
                            .payload
                            .track_metadata
                            .additional_info
                            .recording_mbid
                            .is_empty()
                        {
                            eprintln!(
                                "This song is unknown to ListenBrainz, and \
                                 cannot be rated"
                            );
                        }

                        let feedback = LoveHate {
                            recording_mbid: &data
                                .payload
                                .track_metadata
                                .additional_info
                                .recording_mbid,
                            score,
                        };

                        if !data.online {
                            eprintln!("You must be online to submit feedback");
                            continue;
                        }

                        let status = ureq::post(
                            "https://api.listenbrainz.org/1/feedback/recording-feedback",
                        )
                        .set("Authorization", &data.token)
                        .send_json(feedback);

                        if status.is_err() {
                            eprintln!("Error submitting feedback: {:?}", status);
                        } else {
                            eprintln!("Feedback submitted successfully");
                        }
                    }
                }
                Some(Ok(Event::PropertyChange { name, change, .. })) => {
                    if name == "pause" && data.scrobble {
                        let PropertyData::Flag(paused) = change else {
                            unreachable!();
                        };

                        if paused {
                            data.pause_instant = Instant::now();
                            rx_handle.remove(timer);
                        } else {
                            data.scrobble_deadline =
                                data.scrobble_deadline + data.pause_instant.elapsed();
                            timer = rx_handle
                                .insert_source(
                                    Timer::from_deadline(data.scrobble_deadline),
                                    timer_event,
                                )
                                .unwrap();
                        }
                    } else if name == "speed" && data.scrobble {
                        let PropertyData::Double(speed) = change else {
                            unreachable!();
                        };

                        let duration = mpv.get_property::<f64>("duration").unwrap();
                        let pos = mpv.get_property::<f64>("time-pos").unwrap();
                        data.scrobble_deadline = Instant::now()
                            + Duration::from_secs_f64(scrobble_duration!(duration, speed) - pos);
                        data.payload.track_metadata.additional_info.duration_ms =
                            (duration * 1000.0) as u64;
                        timer = rx_handle
                            .insert_source(
                                Timer::from_deadline(data.scrobble_deadline),
                                timer_event,
                            )
                            .unwrap();
                    }
                }
                Some(Ok(Event::Seek)) => {
                    if mpv.get_property::<i64>("time-pos").unwrap() == 0 {
                        rx_handle.remove(timer);
                        let duration = mpv.get_property::<f64>("duration").unwrap();
                        let speed = mpv.get_property::<f64>("speed").unwrap();

                        data.scrobble_deadline = Instant::now()
                            + Duration::from_secs_f64(scrobble_duration!(duration, speed));
                        data.payload.track_metadata.additional_info.duration_ms =
                            (duration * 1000.0) as u64;
                        timer = rx_handle
                            .insert_source(
                                Timer::from_deadline(data.scrobble_deadline),
                                timer_event,
                            )
                            .unwrap();
                    }
                }
                Some(Ok(Event::FileLoaded)) => {
                    let audio_pts: Result<i64, libmpv::Error> = mpv.get_property("audio-pts");
                    if audio_pts.is_err() || audio_pts.unwrap() < 1 {
                        rx_handle.remove(timer);

                        data.payload.track_metadata.additional_info.release_mbid = String::new();
                        data.payload.track_metadata.additional_info.artist_mbids = Vec::new();
                        data.payload.track_metadata.additional_info.recording_mbid = String::new();
                        data.payload.track_metadata.artist_name = String::new();
                        data.payload.track_metadata.track_name = String::new();
                        data.payload.track_metadata.release_name = String::new();
                        for i in mpv
                            .get_property::<libmpv::MpvNode>("metadata")
                            .unwrap()
                            .to_map()
                            .unwrap()
                        {
                            #[cfg(debug_assertions)]
                            dbg!(i.0);
                            match i.0 {
                                "MUSICBRAINZ_ALBUMID" | "MusicBrainz Album Id" => {
                                    data.payload.track_metadata.additional_info.release_mbid =
                                        i.1.to_str().unwrap().to_string()
                                }
                                "MUSICBRAINZ_ARTISTID" | "MusicBrainz Artist Id" => {
                                    let artists = i.1.to_str().unwrap();

                                    #[cfg(debug_assertions)]
                                    dbg!(artists);

                                    data.payload.track_metadata.additional_info.artist_mbids =
                                        if memchr::memchr(b';', artists.as_bytes()).is_some() {
                                            i.1.to_str()
                                                .unwrap()
                                                .split(";")
                                                .map(|f| f.trim().to_string())
                                                .collect()
                                        } else {
                                            i.1.to_str()
                                                .unwrap()
                                                .split("/")
                                                .map(|f| f.trim().to_string())
                                                .collect()
                                        };
                                }
                                "MUSICBRAINZ_TRACKID" | "http://musicbrainz.org" => {
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

                        #[cfg(debug_assertions)]
                        {
                            dbg!(
                                *mpv.get_property::<MpvStr>("filename").unwrap()
                                    != data.payload.track_metadata.track_name
                            );
                            dbg!(!data.payload.track_metadata.artist_name.is_empty());
                            dbg!(!data.payload.track_metadata.track_name.is_empty());
                            dbg!(!data.payload.track_metadata.release_name.is_empty());
                            #[cfg(feature = "only-scrobble-if-mbid")]
                            dbg!(!data
                                .payload
                                .track_metadata
                                .additional_info
                                .release_mbid
                                .is_empty());
                        }

                        data.scrobble = (*mpv.get_property::<MpvStr>("filename").unwrap()
                            != data.payload.track_metadata.track_name)
                            && !data.payload.track_metadata.artist_name.is_empty()
                            && !data.payload.track_metadata.track_name.is_empty()
                            && !data.payload.track_metadata.release_name.is_empty();

                        #[cfg(feature = "only-scrobble-if-mbid")]
                        {
                            data.scrobble = data.scrobble
                                && !data
                                    .payload
                                    .track_metadata
                                    .additional_info
                                    .release_mbid
                                    .is_empty();
                        }

                        if data
                            .payload
                            .track_metadata
                            .additional_info
                            .recording_mbid
                            .is_empty()
                        {
                            let filename: MpvStr = mpv.get_property("path").unwrap();

                            let _ = read_recording_id(&filename, data);
                        }

                        if data.scrobble {
                            let duration = mpv.get_property::<f64>("duration").unwrap();
                            let speed = mpv.get_property::<f64>("speed").unwrap();

                            data.scrobble_deadline = Instant::now()
                                + Duration::from_secs_f64(scrobble_duration!(duration, speed));
                            data.payload.track_metadata.additional_info.duration_ms =
                                (duration * 1000.0) as u64;
                            timer = rx_handle
                                .insert_source(
                                    Timer::from_deadline(data.scrobble_deadline),
                                    timer_event,
                                )
                                .unwrap();

                            if data.online {
                                data.payload.listened_at = None;
                                scrobble(
                                    "playing_now",
                                    &data.payload,
                                    data.online,
                                    &data.token,
                                    &data.cache_path,
                                );
                            }
                        }
                    }
                }
                None => break,
                _ => {}
            }
        })
        .unwrap();

    data.online = true;

    #[cfg(feature = "connman")]
    {
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

            let state = properties
                .get("State")
                .unwrap()
                .0
                .as_str()
                .unwrap_or_default();

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
                                    import_cache(&data.token, &data.cache_path);
                                    if data.scrobble {
                                        data.payload.listened_at = None;
                                        scrobble(
                                            "playing_now",
                                            &data.payload,
                                            data.online,
                                            &data.token,
                                            &data.cache_path,
                                        );
                                    }
                                }
                            }
                        }
                    }
                    None
                })
                .unwrap();
            state == "ready" || state == "online"
        };
    }
    drop(handle);

    if data.online {
        import_cache(&data.token, &data.cache_path);
    }

    event_loop.run(None, &mut data, |_| {}).unwrap();
    return 0;
}
