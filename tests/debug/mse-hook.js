(function () {
  var lines = []; var log = function () { try { var m = Array.prototype.slice.call(arguments).join(' '); lines.push(m); window.__mse = lines; } catch (e) {} };
  if (window === window.top) setInterval(function () { var v = document.querySelector('video'); var vs = v ? ('t=' + v.currentTime.toFixed(1) + ' rs=' + v.readyState + ' ns=' + v.networkState + ' dur=' + v.duration + ' buf=' + (v.buffered.length ? v.buffered.start(0).toFixed(1) + '-' + v.buffered.end(0).toFixed(1) : 'none') + ' paused=' + v.paused + ' seeking=' + v.seeking + ' rate=' + v.playbackRate + ' muted=' + v.muted + ' err=' + (v.error && v.error.code)) : 'novideo'; document.title = 'DBG ' + vs + ' | ' + lines.slice(-6).join(' ; '); }, 500);
  if (!window.MediaSource) { log('no MediaSource'); return; }
  var its = MediaSource.isTypeSupported.bind(MediaSource);
  MediaSource.isTypeSupported = function (t) { return its(t); };
  var asb = MediaSource.prototype.addSourceBuffer;
  MediaSource.prototype.addSourceBuffer = function (t) { log('addSourceBuffer', t); try { return asb.call(this, t); } catch (e) { log('addSourceBuffer threw', e); throw e; } };
  var ab = SourceBuffer.prototype.appendBuffer; var n = 0;
  SourceBuffer.prototype.appendBuffer = function (d) { n++; if (n < 8 || n % 20 == 0) log('appendBuffer #' + n, d.byteLength); try { return ab.call(this, d); } catch (e) { log('appendBuffer threw', e); throw e; } };
  var cps = HTMLMediaElement.prototype.canPlayType;
  HTMLMediaElement.prototype.canPlayType = function (t) { return cps.call(this, t); };
  document.addEventListener('error', function (e) { log('error event', e.target && e.target.tagName, e.target && e.target.error && e.target.error.code); }, true);
  ['loadedmetadata','canplay','playing','waiting','seeking','seeked','error','stalled','emptied','pause','play','ended','timeupdate'].forEach(function (ev) {
    document.addEventListener(ev, function (e) { if (e.target && e.target.tagName == 'VIDEO' && (ev != 'timeupdate' || Math.random() < 0.05)) log('video', ev, 't=' + e.target.currentTime, 'rs=' + e.target.readyState); }, true);
  });
  window.addEventListener('error', function (e) { log('window error', e.message); });
  window.addEventListener('unhandledrejection', function (e) { log('unhandled rejection', e.reason && (e.reason.message || e.reason)); });
})();
