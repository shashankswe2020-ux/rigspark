"use strict";

// Coding-agent connections and, inside the desktop app, launch-at-login and verified updates.
(() => {
  const agentSelect = document.querySelector("#connect-agent");
  const output = document.querySelector("#connect-output");
  const copy = document.querySelector("#connect-copy");
  const desktop = globalThis.llmupDesktop;

  async function loadAgents() {
    if (!agentSelect) return;
    try {
      const response = await globalThis.fetch("/api/connect");
      if (!response.ok) return;
      const data = await response.json();
      agentSelect.replaceChildren();
      for (const agent of Array.isArray(data.agents) ? data.agents : []) {
        if (typeof agent.id !== "string" || typeof agent.label !== "string") continue;
        const option = document.createElement("option");
        option.value = agent.id;
        option.textContent = agent.label;
        agentSelect.append(option);
      }
      await showAgent();
    } catch {
      // Connection help is optional; the rest of the workspace stays usable.
    }
  }

  async function showAgent() {
    if (!agentSelect?.value || !output) return;
    copy.hidden = true;
    try {
      const response = await globalThis.fetch(`/api/connect/${encodeURIComponent(agentSelect.value)}`);
      const data = await response.json().catch(() => ({}));
      if (!response.ok) {
        output.textContent = typeof data.error === "string" ? data.error : "Start a model, then choose an agent.";
        return;
      }
      if (data.kind === "launch" && typeof data.command === "string") {
        output.textContent = `Run in a terminal:\n\n  ${data.command}`;
        copy.dataset.copy = data.command;
        copy.hidden = false;
      } else if (data.kind === "instructions" && typeof data.text === "string") {
        output.textContent = data.text;
        copy.dataset.copy = data.text;
        copy.hidden = false;
      }
    } catch {
      output.textContent = "Connection details are unavailable.";
    }
  }

  agentSelect?.addEventListener("change", showAgent);
  copy?.addEventListener("click", async () => {
    try {
      await globalThis.navigator.clipboard.writeText(copy.dataset.copy || "");
      copy.textContent = "Copied";
      globalThis.setTimeout(() => (copy.textContent = "Copy"), 1500);
    } catch {
      copy.textContent = "Copy failed";
    }
  });
  // The active model changes outside this panel; refresh when the view is opened.
  document.addEventListener("click", (event) => {
    if (event.target instanceof Element && event.target.closest('[data-view="tools"]')) showAgent();
  });
  loadAgents();

  if (!desktop || typeof desktop.status !== "function") return;

  const settings = document.querySelector("#desktop-settings");
  const login = document.querySelector("#launch-at-login");
  const version = document.querySelector("#desktop-version");
  const updateButton = document.querySelector("#desktop-update");
  const updateStatus = document.querySelector("#desktop-update-status");
  const updateLink = document.querySelector("#update-link");
  if (settings) settings.hidden = false;

  async function refreshStatus() {
    try {
      const status = await desktop.status();
      if (version && typeof status.version === "string") version.textContent = status.version;
      if (login) login.checked = status.launchAtLogin === true;
    } catch {
      if (login) login.disabled = true;
    }
  }

  login?.addEventListener("change", async () => {
    login.disabled = true;
    try {
      login.checked = (await desktop.setLaunchAtLogin(login.checked)) === true;
    } catch (error) {
      login.checked = !login.checked;
      if (updateStatus) updateStatus.textContent = String(error);
    } finally {
      login.disabled = false;
    }
  });

  async function checkForUpdate() {
    if (!updateButton || !updateStatus) return;
    updateButton.disabled = true;
    updateStatus.textContent = "Checking for updates…";
    try {
      const update = await desktop.checkForUpdate();
      if (!update) {
        updateStatus.textContent = "RigSpark is up to date.";
        return;
      }
      updateStatus.textContent = `RigSpark ${update.version} is available.`;
      if (globalThis.confirm(`Install RigSpark ${update.version} and restart? Running models keep serving until RigSpark restarts.`)) {
        updateStatus.textContent = "Downloading and verifying the update…";
        await desktop.installUpdate();
      }
    } catch (error) {
      updateStatus.textContent = String(error);
    } finally {
      updateButton.disabled = false;
    }
  }

  updateButton?.addEventListener("click", checkForUpdate);
  // The release link becomes an in-app, signature-verified install.
  updateLink?.addEventListener("click", (event) => {
    event.preventDefault();
    checkForUpdate();
  });
  globalThis.addEventListener("rigspark:check-update", checkForUpdate);
  refreshStatus();
})();
