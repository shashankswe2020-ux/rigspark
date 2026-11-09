// First-run notice: the licence terms, warranty disclaimer and user responsibilities, shown
// until the person confirms them, and again whenever the notice version changes.
document.addEventListener("DOMContentLoaded", () => {
  const dialog = document.querySelector("#legal-notice");
  const accept = document.querySelector("#legal-accept");
  if (!dialog || !accept || typeof dialog.showModal !== "function") return;
  const KEY = "rigspark.notice";
  const VERSION = "1";

  function seen() {
    try {
      return globalThis.localStorage.getItem(KEY) === VERSION;
    } catch {
      return false;
    }
  }
  function remember() {
    try {
      globalThis.localStorage.setItem(KEY, VERSION);
    } catch {
      // Without storage the notice simply shows again next time.
    }
  }
  function open() {
    if (!dialog.open) dialog.showModal();
    // Start at the top so the first term is read first, even on a small screen.
    accept.focus({ preventScroll: true });
    dialog.scrollTop = 0;
  }

  accept.addEventListener("click", () => {
    remember();
    dialog.close();
  });
  // Escape does not skip the first reading; once confirmed, it closes as usual.
  dialog.addEventListener("cancel", (event) => {
    if (!seen()) event.preventDefault();
  });
  document.querySelector("#legal-open")?.addEventListener("click", open);
  if (!seen()) open();
});
