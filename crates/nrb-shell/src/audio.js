// Звук страницы (встроенный пользовательский скрипт, во всех фреймах, при создании документа).
//  * политика автовоспроизведения: без пользовательского жеста звучащий ролик не стартует (play() отклоняется как в других
//    браузерах, атрибут autoplay сразу ставится на паузу); ролики с muted и без звука не затрагиваются;
//  * отключение звука вкладки: все медиаэлементы принудительно muted, страница видит своё значение;
//  * состояние для индикатора вкладки: играет ли что-то со звуком.
// Не покрывает WebAudio (AudioContext) и звук внутри чужих фреймов для индикатора — только сам фрейм.
(function () {
  if (window.__nrbAudio) return;
  var cfg = { blockAutoplay: true, tabMuted: false };
  var activated = false;
  ['pointerdown', 'mousedown', 'keydown', 'touchstart', 'click'].forEach(function (name) {
    window.addEventListener(name, function (e) { if (e.isTrusted) activated = true; }, true);
  });

  var proto = window.HTMLMediaElement && HTMLMediaElement.prototype;
  if (!proto) return;
  var origPlay = proto.play;
  var mutedDesc = Object.getOwnPropertyDescriptor(proto, 'muted');
  var wanted = new WeakMap();      // что страница хочет для muted, пока вкладка заглушена
  var seen = [];                   // элементы, не попавшие в DOM (new Audio()), но игравшие

  function realMuted(el) { return mutedDesc.get.call(el); }
  function remember(el) { if (seen.indexOf(el) < 0) seen.push(el); }
  function blocked(el) { return cfg.blockAutoplay && !activated && !realMuted(el) && el.volume > 0; }
  function silence(el) {
    if (!wanted.has(el)) wanted.set(el, realMuted(el));
    mutedDesc.set.call(el, true);
  }
  function restore(el) {
    if (wanted.has(el)) { mutedDesc.set.call(el, wanted.get(el)); wanted.delete(el); }
  }
  function all() {
    var list = Array.prototype.slice.call(document.querySelectorAll('video,audio'));
    seen.forEach(function (el) { if (list.indexOf(el) < 0) list.push(el); });
    return list;
  }

  proto.play = function () {
    if (cfg.tabMuted) silence(this);
    if (blocked(this)) {
      return Promise.reject(new DOMException("play() failed because the user didn't interact with the document first.", 'NotAllowedError'));
    }
    remember(this);
    return origPlay.apply(this, arguments);
  };
  // autoplay-атрибут не вызывает play(): ловим событие.
  document.addEventListener('play', function (e) {
    var el = e.target;
    if (!(el instanceof HTMLMediaElement)) return;
    remember(el);
    if (cfg.tabMuted) silence(el);
    if (blocked(el)) el.pause();
  }, true);

  if (mutedDesc && mutedDesc.set) {
    Object.defineProperty(proto, 'muted', {
      configurable: true, enumerable: mutedDesc.enumerable,
      get: function () { return wanted.has(this) ? wanted.get(this) : mutedDesc.get.call(this); },
      set: function (value) {
        if (cfg.tabMuted) { wanted.set(this, !!value); mutedDesc.set.call(this, true); }
        else mutedDesc.set.call(this, !!value);
      }
    });
  }

  window.__nrbAudio = {
    config: function (c) {
      if (typeof c.blockAutoplay === 'boolean') cfg.blockAutoplay = c.blockAutoplay;
      if (typeof c.tabMuted === 'boolean' && c.tabMuted !== cfg.tabMuted) {
        cfg.tabMuted = c.tabMuted;
        all().forEach(cfg.tabMuted ? silence : restore);
      }
      return true;
    },
    // «Звучит ли вкладка»: что-то играет и (с учётом желания страницы) не заглушено и не нулевой громкости.
    state: function () {
      var playing = all().some(function (el) {
        var wantsMuted = wanted.has(el) ? wanted.get(el) : realMuted(el);
        return !el.paused && !el.ended && el.readyState >= 2 && !wantsMuted && el.volume > 0;
      });
      return JSON.stringify({ playing: playing, muted: cfg.tabMuted });
    }
  };
})();
