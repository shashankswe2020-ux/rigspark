// Confirmations drawn by the page itself. The desktop app's WebKit view has no handler for
// window.confirm(), which there returns false without showing anything, so every
// "Are you sure?" goes through this <dialog> instead. Resolves true only on the confirm button.
(() => {
  "use strict";

  let dialog = null;

  function build() {
    const el = document.createElement("dialog");
    el.className = "confirm-dialog";
    el.setAttribute("aria-labelledby", "confirm-dialog-title");
    el.setAttribute("aria-describedby", "confirm-dialog-message");
    const title = document.createElement("h2");
    title.id = "confirm-dialog-title";
    const message = document.createElement("p");
    message.id = "confirm-dialog-message";
    const label = document.createElement("label");
    label.className = "confirm-dialog-field";
    const labelText = document.createElement("span");
    const input = document.createElement("input");
    input.type = "text";
    input.maxLength = 200;
    input.autocomplete = "off";
    label.append(labelText, input);
    const actions = document.createElement("div");
    actions.className = "confirm-dialog-actions";
    const cancel = document.createElement("button");
    cancel.type = "button";
    cancel.className = "ghost-btn";
    cancel.value = "cancel";
    const ok = document.createElement("button");
    ok.type = "button";
    ok.value = "confirm";
    actions.append(cancel, ok);
    el.append(title, message, label, actions);
    document.body.append(el);
    return el;
  }

  function confirmAction(options) {
    const settings = typeof options === "string" ? { message: options } : options || {};
    dialog = dialog && dialog.isConnected ? dialog : build();
    if (dialog.open) {
      dialog.close("cancel");
    }
    const [title, message] = [dialog.querySelector("h2"), dialog.querySelector("p")];
    const field = dialog.querySelector(".confirm-dialog-field");
    const input = field.querySelector("input");
    const asking = typeof settings.value === "string";
    field.hidden = !asking;
    field.querySelector("span").textContent = settings.label || "";
    input.value = asking ? settings.value : "";
    message.hidden = !settings.message;
    const [cancel, ok] = dialog.querySelectorAll("button");
    title.textContent = settings.title || "Are you sure?";
    title.hidden = !settings.title;
    message.textContent = settings.message || "";
    cancel.textContent = settings.cancelLabel || "Cancel";
    cancel.hidden = settings.notice === true;
    ok.textContent = settings.confirmLabel || "OK";
    ok.className = settings.danger ? "accent-btn danger-btn" : "accent-btn";
    return new Promise((resolve) => {
      const finish = (value) => {
        cancel.removeEventListener("click", onCancel);
        ok.removeEventListener("click", onConfirm);
        dialog.removeEventListener("close", onClose);
        input.removeEventListener("keydown", onKey);
        if (dialog.open) {
          dialog.close();
        }
        resolve(value);
      };
      const onCancel = () => finish(asking ? null : false);
      const onConfirm = () => finish(asking ? input.value : true);
      // Escape closes the dialog: that is a "no".
      const onClose = () => finish(asking ? null : false);
      const onKey = (event) => {
        if (event.key === "Enter" && asking) {
          event.preventDefault();
          onConfirm();
        }
      };
      cancel.addEventListener("click", onCancel);
      ok.addEventListener("click", onConfirm);
      dialog.addEventListener("close", onClose);
      input.addEventListener("keydown", onKey);
      dialog.showModal();
      if (asking) {
        input.focus();
        input.select();
        return;
      }
      // A destructive action starts on Cancel, so Return never deletes by accident.
      (settings.danger && !settings.notice ? cancel : ok).focus();
    });
  }

  /** Asks before an action; resolves true only when the person confirms. */
  globalThis.rigsparkConfirm = confirmAction;
  /** Asks for a line of text; resolves with it, or null when cancelled (like window.prompt). */
  globalThis.rigsparkPrompt = (options) =>
    confirmAction({ confirmLabel: "Save", ...options, value: options.value ?? "" });
  /** Tells the person something went wrong; resolves when they dismiss it. */
  globalThis.rigsparkNotice = (message, title) =>
    confirmAction({ title, message, confirmLabel: "OK", notice: true }).then(() => undefined);
})();
