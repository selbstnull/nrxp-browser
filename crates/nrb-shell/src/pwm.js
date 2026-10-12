// Менеджер паролей (встроенный пользовательский скрипт, во всех фреймах).
// Делает две вещи и ничего не хранит: (1) замечает отправку формы входа и кладёт введённые логин и пароль в одну ячейку
// `pending`, которую оболочка забирает опросом `poll()`; (2) по команде оболочки подставляет логин и пароль в поля `fill()`.
// Пароль подставляется только по явному действию пользователя (кнопка/сочетание), не при загрузке страницы: иначе его
// прочитал бы любой скрипт страницы ещё до взаимодействия. Страница видит `window.__nrbPw` (и может вызвать `poll()`), но
// ввод пароля пользователем она и так получает сама при отправке формы — новых данных это не раскрывает.
(function () {
  if (window.__nrbPw) return;
  var pending = null;

  function visible(el) {
    if (!el || !el.getClientRects) return false;
    if (!el.getClientRects().length) return false;
    var style = window.getComputedStyle(el);
    return style.visibility !== 'hidden' && style.display !== 'none';
  }
  function passwordFields(root) {
    return Array.prototype.filter.call((root || document).querySelectorAll('input[type=password]'), visible);
  }
  var TEXTY = ['text', 'email', 'tel', 'search', 'url', ''];
  // Поле логина — ближайшее ПЕРЕД паролем видимое текстовое поле той же формы (или документа).
  function usernameFor(pw) {
    var scope = pw.form || document, previous = null;
    var inputs = scope.querySelectorAll('input');
    for (var i = 0; i < inputs.length; i++) {
      if (inputs[i] === pw) break;
      var type = (inputs[i].type || 'text').toLowerCase();
      if (TEXTY.indexOf(type) >= 0 && visible(inputs[i])) previous = inputs[i];
    }
    return previous;
  }
  // Из нескольких заполненных паролей (смена пароля: старый/новый/повтор) берём «новый»: autocomplete=new-password, иначе последний.
  function chosenPassword(root) {
    var filled = passwordFields(root).filter(function (f) { return f.value; });
    if (!filled.length) return null;
    var fresh = filled.filter(function (f) { return (f.autocomplete || '').indexOf('new-password') >= 0; });
    return (fresh.length ? fresh : filled)[fresh.length ? 0 : filled.length - 1];
  }
  function capture(root) {
    var pw = chosenPassword(root);
    if (!pw) return;
    var user = usernameFor(pw);
    pending = { origin: location.origin, username: user ? user.value : '', password: pw.value };
  }

  // Классическая отправка формы; только настоящие (доверенные) события: скрипт страницы не должен подкладывать «ввод».
  document.addEventListener('submit', function (e) {
    if (e.isTrusted) capture(e.target);
  }, true);
  // Современные формы входа отправляют данные через fetch без события submit: ловим Enter в поле пароля и клик по кнопке рядом.
  document.addEventListener('keydown', function (e) {
    if (e.isTrusted && e.key === 'Enter' && e.target && e.target.type === 'password') capture(e.target.form || document);
  }, true);
  document.addEventListener('click', function (e) {
    if (!e.isTrusted) return;
    var button = e.target && e.target.closest && e.target.closest('button, input[type=submit], input[type=button], [role=button]');
    if (button) capture(button.form || document);
  }, true);

  function setValue(el, value) {
    // Через родной сеттер: фреймворки (React и др.) перехватывают `value` на самом элементе.
    var setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    setter.call(el, value);
    el.dispatchEvent(new Event('input', { bubbles: true }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
  }

  window.__nrbPw = {
    // Раз в опрос оболочки: есть ли на странице форма с паролем и что ввёл пользователь с прошлого опроса.
    poll: function () {
      var result = JSON.stringify({ login: passwordFields().length > 0, pending: pending });
      pending = null;
      return result;
    },
    fill: function (username, password) {
      var fields = passwordFields();
      if (!fields.length) return false;
      var pw = fields[0], user = usernameFor(pw);
      if (user && username) setValue(user, username);
      setValue(pw, password);
      return true;
    }
  };
})();
