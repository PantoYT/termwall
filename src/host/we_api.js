// termwall host: the parts of Wallpaper Engine's web API the page uses, so index.html works the
// same here as in WE. The host calls window.__termwallHost.* with what it captures (audio as 128
// floats, 64 per channel from bass up; media events with WE's field names).
(() => {
  if (window.wallpaperRegisterAudioListener) return;  // inside WE after all: leave its API alone
  const L = {audio: [], media: [], playback: [], timeline: [], thumbnail: [], status: []};
  const last = {};  // the latest media events: a listener registered later still hears them
  const add = k => f => {
    if (typeof f !== "function") return;
    L[k].push(f);
    if (k in last) { try { f(last[k]); } catch (e) { /* the page's own error */ } }
  };
  const call = (k, v) => {
    if (k !== "audio") last[k] = v;
    for (const f of L[k]) { try { f(v); } catch (e) { /* the page's own error */ } }
  };
  window.wallpaperMediaIntegration = {PLAYBACK_STOPPED: 0, PLAYBACK_PLAYING: 1, PLAYBACK_PAUSED: 2};
  window.wallpaperRegisterAudioListener = add("audio");
  window.wallpaperRegisterMediaPropertiesListener = add("media");
  window.wallpaperRegisterMediaPlaybackListener = add("playback");
  window.wallpaperRegisterMediaTimelineListener = add("timeline");
  window.wallpaperRegisterMediaThumbnailListener = add("thumbnail");
  window.wallpaperRegisterMediaStatusListener = add("status");
  window.__termwallHost = {
    audio: a => call("audio", a),
    media: e => call("media", e),
    playback: e => call("playback", e),
    timeline: e => call("timeline", e),
    thumbnail: e => call("thumbnail", e),
    status: e => call("status", e),
  };
})();
