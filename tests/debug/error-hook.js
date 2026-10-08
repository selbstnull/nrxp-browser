// Отладочный хук: все необработанные ошибки страницы и отклонённые промисы — в консоль (её читает `nrb snap --json`).
// Использование: NRB_DEBUG_SCRIPT=tests/debug/error-hook.js nrb snap <url> -o x.png --json
(function () {
  var say = function (kind, text) { try { console.error('[' + kind + '] ' + text); } catch (_) {} };
  window.addEventListener('error', function (e) {
    say('onerror', e.message + ' @' + e.filename + ':' + e.lineno + ':' + e.colno + (e.error && e.error.stack ? '\n' + e.error.stack : ''));
  });
  window.addEventListener('unhandledrejection', function (e) {
    var r = e.reason;
    say('unhandled', (r && (r.stack || r.message)) || String(r));
  });
})();
