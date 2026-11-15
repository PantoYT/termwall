//! Now playing: what Windows' media session reports (Spotify, a browser tab, any player with the
//! media keys), as the events Wallpaper Engine's media integration sends a web wallpaper.

/// Wallpaper Engine's playback states (we_api.js defines the same numbers for the page).
pub const STOPPED: u8 = 0;
pub const PLAYING: u8 = 1;
pub const PAUSED: u8 = 2;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Now {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub state: u8,
}

/// The calls for one state: the properties event (an empty title clears the page's line) and
/// the playback event.
pub fn script(now: &Now) -> String {
    let props = serde_json::json!({
        "title": now.title, "artist": now.artist, "subTitle": "", "albumTitle": now.album,
        "albumArtist": now.album_artist, "genres": "", "contentType": "music",
    });
    format!(
        "window.__termwallHost&&(window.__termwallHost.status({{enabled:true}}),window.__termwallHost.media({props}),\
window.__termwallHost.playback({{state:{}}}))",
        now.state
    )
}

#[cfg(windows)]
pub mod watch {
    //! Asked once a second (a handful of cheap calls) rather than through event handlers: the
    //! session comes and goes with the players, and every one would need its handlers rewired.

    use std::time::{Duration, Instant};

    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSessionManager as Manager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
    };
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

    use super::{Now, PAUSED, PLAYING, STOPPED};

    fn read(manager: &Manager) -> Now {
        // no session, or one that is closed or stopped: nothing is playing
        let Ok(session) = manager.GetCurrentSession() else { return Now::default() };
        let state = match session.GetPlaybackInfo().and_then(|i| i.PlaybackStatus()) {
            Ok(Status::Playing) | Ok(Status::Changing) => PLAYING,
            Ok(Status::Paused) => PAUSED,
            _ => STOPPED,
        };
        if state == STOPPED {
            return Now::default();
        }
        let Ok(p) = session.TryGetMediaPropertiesAsync().and_then(|op| op.join()) else { return Now::default() };
        let text = |r: windows::core::Result<windows::core::HSTRING>| r.map(|h| h.to_string()).unwrap_or_default();
        Now {
            title: text(p.Title()),
            artist: text(p.Artist()),
            album: text(p.AlbumTitle()),
            album_artist: text(p.AlbumArtist()),
            state,
        }
    }

    /// `on_change` gets every change, and the current state again every ten seconds (a page
    /// that reloaded itself has forgotten it).
    pub fn spawn(on_change: impl Fn(&Now) + Send + 'static) {
        let run = move || {
            // SAFETY: COM for this thread only.
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
            let mut manager: Option<Manager> = None;
            let (mut last, mut sent) = (None::<Now>, Instant::now());
            loop {
                if manager.is_none() {
                    manager = Manager::RequestAsync().and_then(|op| op.join()).ok();
                }
                if let Some(m) = &manager {
                    let now = read(m);
                    if last.as_ref() != Some(&now) || sent.elapsed() >= Duration::from_secs(10) {
                        on_change(&now);
                        last = Some(now);
                        sent = Instant::now();
                    }
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        };
        let _ = std::thread::Builder::new().name("termwall-media".into()).spawn(run);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_events_for_the_page() {
        let now =
            Now { title: "He said \"hi\" </script>".into(), artist: "Zażółć".into(), state: PLAYING, ..Default::default() };
        let s = script(&now);
        assert!(s.contains(r#""title":"He said \"hi\" </script>""#) && s.contains(r#""artist":"Zażółć""#), "{s}");
        assert!(s.contains("playback({state:1})") && s.starts_with("window.__termwallHost&&("));
        assert!(script(&Now::default()).contains(r#""title":"""#), "nothing playing clears the line");
    }
}
