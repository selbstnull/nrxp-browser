// Совместимость с сайтами (встроенный пользовательский скрипт, во всех фреймах, до скриптов страницы).
//
// `document.all`: в Servo его нет (`document.all === undefined`, `'all' in document === false`). Polymer Resin на YouTube
// проверяет значение привязки как `v !== document.all` — при отсутствии `document.all` любое `undefined` «равно» ему и
// заменяется строкой "zClosurez": `hidden="[[data.hideContents]]"` становится истинным, и список результатов поиска скрыт.
// Подставляем живую коллекцию всех элементов (как в IE): объект никогда не равен значению привязки. Отличие от браузеров —
// `typeof document.all === "object"` и значение истинно (в настоящих браузерах — «undefined»/ложно); старый код,
// определяющий IE по `document.all`, может принять Servo за IE.
(function () {
  if (typeof Document === 'undefined' || document.all !== undefined) return;
  var cache = typeof WeakMap === 'function' ? new WeakMap() : null;
  try {
    Object.defineProperty(Document.prototype, 'all', {
      configurable: true,
      enumerable: false,
      get: function () {
        var doc = this;
        var hit = cache && cache.get(doc);
        if (hit) return hit;
        var all = doc.getElementsByTagName('*');
        if (cache) cache.set(doc, all);
        return all;
      },
    });
  } catch (e) {}
})();

// `navigator.mediaCapabilities`: в Servo его нет, а YouTube без него не предлагает AV1/высокие разрешения. Отвечаем по тому, что
// умеет наш MSE (`MediaSource.isTypeSupported`) и `canPlayType`; «плавно» — для всего поддерживаемого, «энергоэффективно» —
// для H.264 (аппаратное декодирование).
(function () {
  if (typeof Navigator === 'undefined' || navigator.mediaCapabilities) return;
  function info(config) {
    config = config || {};
    var types = [];
    if (config.video && config.video.contentType) types.push(String(config.video.contentType));
    if (config.audio && config.audio.contentType) types.push(String(config.audio.contentType));
    if (!types.length) return Promise.reject(new TypeError('decodingInfo: no configuration'));
    var mse = config.type === 'media-source';
    var supported = types.every(function (t) {
      try {
        if (mse && window.MediaSource) return MediaSource.isTypeSupported(t);
        return document.createElement('video').canPlayType(t) !== '';
      } catch (e) { return false; }
    });
    var hw = types.some(function (t) { return /avc1|avc3/i.test(t); });
    return Promise.resolve({ supported: supported, smooth: supported, powerEfficient: supported && hw });
  }
  var caps = { decodingInfo: info, encodingInfo: function () { return Promise.resolve({ supported: false, smooth: false, powerEfficient: false }); } };
  try {
    Object.defineProperty(Navigator.prototype, 'mediaCapabilities', { configurable: true, enumerable: true, get: function () { return caps; } });
  } catch (e) {}
})();
