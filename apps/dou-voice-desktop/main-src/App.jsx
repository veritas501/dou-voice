import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { motion } from "framer-motion";

import { command, listen } from "../web/scripts/tauri-api.js";

const DEFAULT_HOTKEY = "Ctrl+Q";
const MODIFIER_KEYS = ["Control", "Alt", "Shift", "Meta"];
const META_KEY_LABEL = /Mac|iPhone|iPad|iPod/.test(window.navigator.platform)
  ? "Command"
  : "Win";

const phaseLabels = {
  idle: "Idle",
  starting: "Starting",
  loading_auth: "Loading auth",
  recording: "Recording",
  stopping: "Stopping",
  recognizing: "Recognizing",
  typing: "Typing",
  error: "Error",
};

const inputMethodLabels = {
  direct: "Direct typing with fallback",
  clipboardPaste: "Clipboard paste",
};

function App() {
  const [voiceStatus, setVoiceStatus] = useState({
    phase: "idle",
    message: "Ready.",
    lastText: "",
  });
  const [settings, setSettings] = useState(defaultSettings());
  const [auth, setAuth] = useState(null);
  const [authPath, setAuthPath] = useState("");
  const [appIcon, setAppIcon] = useState("");
  const [buildInfo, setBuildInfo] = useState(null);
  const [devices, setDevices] = useState([{ id: "default", name: "Default", isDefault: true }]);
  const [logLines, setLogLines] = useState(["Ready. Use Export for detailed diagnostics."]);
  const [capturingHotkey, setCapturingHotkey] = useState(false);
  const [pendingModifierHotkey, setPendingModifierHotkey] = useState(null);
  const [awaitingHotkeyRelease, setAwaitingHotkeyRelease] = useState(false);
  const [currentSection, setCurrentSection] = useState("general");
  const [onboardingRequired, setOnboardingRequired] = useState(null);
  const [onboardingStep, setOnboardingStep] = useState(0);
  const [onboardingDraft, setOnboardingDraft] = useState(defaultSettings());
  const previousHotkeyRef = useRef(DEFAULT_HOTKEY);
  const settingsRef = useRef(settings);
  const logRef = useRef(null);

  useEffect(() => {
    settingsRef.current = settings;
  }, [settings]);

  const writeLog = useCallback((message) => {
    const time = new Date().toLocaleTimeString();
    setLogLines((lines) => [...lines, `[${time}] ${message}`]);
  }, []);

  useEffect(() => {
    logRef.current?.scrollTo({ top: logRef.current.scrollHeight });
  }, [logLines]);

  const renderAuthStatus = useMemo(() => authStatusLabel(auth), [auth]);
  const selectedDevice = settings.selectedInputDevice || "default";
  const deviceOptions = useMemo(
    () => withSelectedDevice(devices, selectedDevice),
    [devices, selectedDevice],
  );

  const saveSettings = useCallback(
    async (nextSettings, message = "Settings saved.") => {
      const sanitized = normalizeSettings(nextSettings);
      const snapshot = await command("save_settings", { settings: sanitized });
      const saved = normalizeSettings(snapshot.settings || sanitized);
      setSettings(saved);
      setOnboardingDraft(saved);
      setAuth(snapshot.auth || null);
      writeLog(
        `${message} hotkey=${saved.hotkey} input=${saved.inputMethod} mic=${
          saved.selectedInputDevice || "default"
        }`,
      );
      return saved;
    },
    [writeLog],
  );

  useEffect(() => {
    command("get_default_auth_path")
      .then((path) => {
        setAuthPath(path);
        writeLog(`Default auth path: ${path}`);
      })
      .catch((error) => writeLog(`Default auth path failed: ${error}`));

    command("get_app_icon_data_url")
      .then(setAppIcon)
      .catch((error) => writeLog(`Read app icon failed: ${error}`));

    command("get_app_build_info")
      .then(setBuildInfo)
      .catch((error) => writeLog(`Read build info failed: ${error}`));

    command("get_voice_status")
      .then((status) => setVoiceStatus(normalizeVoiceStatus(status)))
      .catch((error) => writeLog(`Read status failed: ${error}`));

    command("get_settings")
      .then((snapshot) => {
        const loadedSettings = normalizeSettings(snapshot.settings);
        setSettings(loadedSettings);
        setOnboardingDraft(loadedSettings);
        setAuth(snapshot.auth || null);
        setOnboardingRequired(Boolean(snapshot.onboardingRequired));
      })
      .catch((error) => {
        writeLog(`Read settings failed: ${error}`);
        setOnboardingRequired(false);
      });

    command("get_available_input_devices")
      .then((items) => setDevices(normalizeDevices(items)))
      .catch((error) => writeLog(`Read microphones failed: ${error}`));

    let active = true;
    let unlisten;
    listen("voice-status", (event) => {
      if (active) setVoiceStatus(normalizeVoiceStatus(event.payload));
    }).then((callback) => {
      unlisten = callback;
    });
    return () => {
      active = false;
      unlisten?.();
    };
  }, [writeLog]);

  useEffect(() => {
    const onKeydown = (event) => {
      if (awaitingHotkeyRelease) {
        blockEvent(event);
        return;
      }
      if (!capturingHotkey) return;

      blockEvent(event);
      const hotkey = formatHotkeyEvent(event, {
        onCancel: () => cancelHotkeyCapture(),
      });
      if (!hotkey) return;

      if (isModifierKey(event.key)) {
        setPendingModifierHotkey(hotkey);
        return;
      }

      finalizeHotkey(hotkey, { resumeAfterKeyup: true });
    };

    const onKeyup = (event) => {
      if (awaitingHotkeyRelease) {
        blockEvent(event);
        setAwaitingHotkeyRelease(false);
        command("end_hotkey_capture").catch((error) => writeLog(error));
        return;
      }
      if (!capturingHotkey || !pendingModifierHotkey) return;
      blockEvent(event);
      finalizeHotkey(pendingModifierHotkey);
    };

    document.addEventListener("keydown", onKeydown, true);
    document.addEventListener("keyup", onKeyup, true);
    return () => {
      document.removeEventListener("keydown", onKeydown, true);
      document.removeEventListener("keyup", onKeyup, true);
    };
  }, [
    awaitingHotkeyRelease,
    capturingHotkey,
    onboardingRequired,
    pendingModifierHotkey,
    saveSettings,
    writeLog,
  ]);

  useEffect(() => {
    let active = true;
    let unlisten;
    listen("native-hotkey-capture", (event) => {
      if (!active || !capturingHotkey) return;

      const hotkey = event.payload?.hotkey;
      if (!hotkey) return;

      if (event.payload?.isKeyDown) {
        setPendingModifierHotkey(hotkey);
        return;
      }

      finalizeHotkey(pendingModifierHotkey || hotkey);
    }).then((callback) => {
      unlisten = callback;
    });

    return () => {
      active = false;
      unlisten?.();
    };
  }, [capturingHotkey, pendingModifierHotkey, onboardingRequired, saveSettings, writeLog]);

  async function openLoginWindow() {
    await command("open_login_window");
    writeLog("Login window opened.");
  }

  async function exportAuth() {
    const fallbackPath = await command("get_default_auth_path");
    const result = await command("export_auth", { outputPath: fallbackPath });
    const refreshedAuth = await command("check_auth_status");
    setAuth(refreshedAuth);
    writeLog(
      `Auth exported: ${result.outputPath}\n` +
        `cookie_count=${result.cookieCount}\n` +
        `device_id_present=${result.deviceIdPresent}\n` +
        `web_id_present=${result.webIdPresent}\n` +
        `refreshed_cookie_count=${refreshedAuth.cookieCount || 0}`,
    );
  }

  async function recordOnce() {
    const result = await command("record_once_and_type", { seconds: 5 });
    writeLog(`Voice input completed: ${result.finalText}\npcm_bytes=${result.pcmBytes}`);
  }

  async function exportDiagnostics() {
    const result = await command("export_diagnostics");
    writeLog(`Diagnostics exported: ${result.outputPath}\nevent_count=${result.eventCount}`);
  }

  async function checkAuthStatus() {
    const result = await command("check_auth_status");
    setAuth(result);
    writeLog(`Auth status: load_ok=${result.loadOk} exists=${result.exists} path=${result.path}`);
  }

  async function startHotkeyCapture() {
    if (capturingHotkey || awaitingHotkeyRelease) return;
    await command("begin_hotkey_capture");
    previousHotkeyRef.current = settingsRef.current.hotkey || DEFAULT_HOTKEY;
    setCapturingHotkey(true);
    setPendingModifierHotkey(null);
    writeLog("Press the new hotkey combination.");
  }

  async function useDefaultHotkey() {
    await stopHotkeyCapture();
    if (onboardingRequired) {
      updateOnboardingDraft({ hotkey: DEFAULT_HOTKEY });
      return;
    }
    await saveSettings({ ...settingsRef.current, hotkey: DEFAULT_HOTKEY }, "Default hotkey saved.");
  }

  async function stopHotkeyCapture({ resumeBackend = true } = {}) {
    setCapturingHotkey(false);
    setPendingModifierHotkey(null);
    setAwaitingHotkeyRelease(false);
    if (resumeBackend) {
      await command("end_hotkey_capture").catch((error) => writeLog(error));
    }
  }

  function cancelHotkeyCapture() {
    stopHotkeyCapture();
    setSettings((current) => ({ ...current, hotkey: previousHotkeyRef.current }));
    writeLog("Hotkey capture canceled.");
  }

  async function finalizeHotkey(hotkey, { resumeAfterKeyup = false } = {}) {
    let normalizedHotkey;
    try {
      normalizedHotkey = await command("normalize_hotkey_candidate", { shortcut: hotkey });
    } catch (error) {
      writeLog(`Hotkey is not supported: ${hotkey}. ${error}`);
      return;
    }

    stopHotkeyCapture({ resumeBackend: !resumeAfterKeyup });
    setAwaitingHotkeyRelease(resumeAfterKeyup);
    writeLog(`Hotkey captured: ${normalizedHotkey}`);
    if (onboardingRequired) {
      updateOnboardingDraft({ hotkey: normalizedHotkey });
      return;
    }
    saveSettings({ ...settingsRef.current, hotkey: normalizedHotkey }, "Hotkey saved.").catch((error) =>
      writeLog(error),
    );
  }

  function updateSetting(patch, message) {
    const next = normalizeSettings({ ...settingsRef.current, ...patch });
    setSettings(next);
    saveSettings(next, message).catch((error) => writeLog(error));
  }

  function updateOnboardingDraft(patch) {
    setOnboardingDraft((current) => normalizeSettings({ ...current, ...patch }));
  }

  async function finishOnboarding() {
    await saveSettings(onboardingDraft, "Initial settings saved.");
    setOnboardingRequired(false);
    setCurrentSection("general");
  }

  const displayedHotkey = capturingHotkey
    ? pendingModifierHotkey || "Press hotkey..."
    : settings.hotkey || DEFAULT_HOTKEY;
  const sections = [
    { id: "general", label: "General", hint: "Voice input" },
    { id: "auth", label: "Auth", hint: renderAuthStatus },
    { id: "diagnostics", label: "Diagnostics", hint: "Logs" },
    { id: "about", label: "About", hint: "Status" },
  ];
  const activeSection = sections.find((section) => section.id === currentSection) || sections[0];

  if (onboardingRequired === null) {
    return <OnboardingLoading appIcon={appIcon} />;
  }

  if (onboardingRequired) {
    return (
      <OnboardingWizard
        appIcon={appIcon}
        auth={auth}
        authPath={authPath}
        devices={deviceOptions}
        draft={onboardingDraft}
        displayedHotkey={capturingHotkey ? displayedHotkey : onboardingDraft.hotkey}
        capturingHotkey={capturingHotkey}
        step={onboardingStep}
        onStepChange={setOnboardingStep}
        onOpenLogin={() => runAction(openLoginWindow, writeLog)}
        onExportAuth={() => runAction(exportAuth, writeLog)}
        onCheckAuth={() => runAction(checkAuthStatus, writeLog)}
        onDraftChange={updateOnboardingDraft}
        onHotkeyCapture={startHotkeyCapture}
        onDefaultHotkey={useDefaultHotkey}
        onTestRecording={() => runAction(recordOnce, writeLog)}
        onFinish={() => runAction(finishOnboarding, writeLog)}
      />
    );
  }

  return (
    <main className="app-shell">
      <aside className="sidebar">
        <div className="sidebar-brand">
          <div className="brand-mark">
            {appIcon ? <img src={appIcon} alt="Dou Voice" /> : "DV"}
          </div>
          <div>
            <strong>Dou Voice</strong>
            <span>Speech input</span>
          </div>
        </div>

        <nav className="sidebar-nav" aria-label="Main sections">
          {sections.map((section) => (
            <button
              key={section.id}
              className="sidebar-item"
              data-active={currentSection === section.id}
              type="button"
              onClick={() => setCurrentSection(section.id)}
            >
              <span>{section.label}</span>
              <small>{section.hint}</small>
            </button>
          ))}
        </nav>

        <div className="sidebar-status">
          <StatusPill phase={voiceStatus.phase} />
          <p>{voiceStatus.message || "Ready."}</p>
        </div>
      </aside>

      <section className="main-pane">
        <header className="main-header">
          <div>
            <span className="eyebrow">{activeSection.hint}</span>
            <h1>{activeSection.label}</h1>
          </div>
          <button className="primary" type="button" onClick={() => runAction(recordOnce, writeLog)}>
            Test Recording
          </button>
        </header>

        {currentSection === "general" && (
          <motion.div className="section-stack" initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }}>
            <SettingsGroup title="Live" description="Current transcription state">
              <div className="live-card">
                <div className="live-card-top">
                  <StatusPill phase={voiceStatus.phase} />
                  <span>{voiceStatus.message || "Ready."}</span>
                </div>
                <div className="transcript-box">
                  {voiceStatus.lastText || <span>No transcription yet.</span>}
                </div>
              </div>
            </SettingsGroup>

            <SettingsGroup title="Input" description="Recording and text insertion">
              <SettingRow title="Hotkey" description="Press-to-talk shortcut">
                <div className="hotkey-row compact">
                  <input id="setting-hotkey" readOnly spellCheck="false" value={displayedHotkey} />
                  <button type="button" onClick={startHotkeyCapture} disabled={capturingHotkey}>
                    Change
                  </button>
                  <button type="button" onClick={useDefaultHotkey}>
                    Default
                  </button>
                </div>
              </SettingRow>

              <SettingRow title="Microphone" description="Audio input device">
                <select
                  id="setting-input-device"
                  value={selectedDevice}
                  onChange={(event) =>
                    updateSetting(
                      {
                        selectedInputDevice:
                          event.target.value === "default" ? null : event.target.value,
                      },
                      "Microphone saved.",
                    )
                  }
                >
                  {deviceOptions.map((device) => (
                    <option key={device.id} value={device.id}>
                      {device.name}
                      {device.isDefault && device.id !== "default" ? " (system default)" : ""}
                    </option>
                  ))}
                </select>
              </SettingRow>

              <SettingRow title="Input Method" description="How recognized text is inserted">
                <select
                  id="setting-input-method"
                  value={settings.inputMethod}
                  onChange={(event) =>
                    updateSetting({ inputMethod: event.target.value }, "Input method saved.")
                  }
                >
                  <option value="direct">Direct typing with fallback</option>
                  <option value="clipboardPaste">Clipboard paste</option>
                </select>
              </SettingRow>
            </SettingsGroup>

            <SettingsGroup title="Feedback" description="Desktop feedback surfaces">
              <SettingRow title="Sound" description="Play system feedback sounds">
                <Toggle
                  checked={settings.soundEnabled !== false}
                  title="Sound"
                  subtitle=""
                  onChange={(checked) => updateSetting({ soundEnabled: checked }, "Sound setting saved.")}
                />
              </SettingRow>
              <SettingRow title="Overlay" description="Show compact live status capsule">
                <Toggle
                  checked={settings.overlayEnabled !== false}
                  title="Overlay"
                  subtitle=""
                  onChange={(checked) => updateSetting({ overlayEnabled: checked }, "Overlay setting saved.")}
                />
              </SettingRow>
            </SettingsGroup>
          </motion.div>
        )}

        {currentSection === "auth" && (
          <motion.div className="section-stack" initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }}>
            <SettingsGroup title="Doubao Session" description="Login state and auth export">
              <SettingRow title="Status" description="Current auth file state">
                <div className="inline-actions">
                  <span className="value-pill">{renderAuthStatus}</span>
                  <button type="button" onClick={() => runAction(checkAuthStatus, writeLog)}>
                    Refresh
                  </button>
                </div>
              </SettingRow>
              <SettingRow title="Auth File" description="Where auth.json is stored" layout="stacked">
                <div id="output-path" className="auth-path-display" title={authPath}>
                  {authPath || "Resolving default auth path..."}
                </div>
              </SettingRow>
              <SettingRow title="Login" description="Open Doubao and export current session">
                <div className="inline-actions">
                  <button type="button" onClick={() => runAction(openLoginWindow, writeLog)}>
                    Open Login
                  </button>
                  <button className="primary" type="button" onClick={() => runAction(exportAuth, writeLog)}>
                    Export Auth
                  </button>
                </div>
              </SettingRow>
            </SettingsGroup>
          </motion.div>
        )}

        {currentSection === "diagnostics" && (
          <motion.div className="section-stack" initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }}>
            <SettingsGroup title="Activity" description="Recent local events">
              <div className="diagnostics-body">
                <div className="inline-actions diagnostics-actions">
                  <button type="button" onClick={() => runAction(exportDiagnostics, writeLog)}>
                    Export Diagnostics
                  </button>
                </div>
                <pre ref={logRef} className="activity-log">
                  {logLines.join("\n")}
                </pre>
              </div>
            </SettingsGroup>
          </motion.div>
        )}

        {currentSection === "about" && (
          <motion.div className="section-stack" initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }}>
            <SettingsGroup title="Overview" description="Current runtime configuration">
              <SettingRow title="Version" description="Binary package version">
                <span className="value-pill">{buildInfo?.version || "Unknown"}</span>
              </SettingRow>
              <SettingRow title="Commit" description="Source revision embedded at build time">
                <span className="value-pill">{formatCommit(buildInfo)}</span>
              </SettingRow>
              <SettingRow title="Built" description="Build timestamp">
                <span className="value-pill">{formatBuildTime(buildInfo?.buildUnixMs)}</span>
              </SettingRow>
              <SettingRow title="Hotkey" description="Press-to-talk shortcut">
                <span className="value-pill">{displayedHotkey}</span>
              </SettingRow>
              <SettingRow title="Microphone" description="Selected recording input">
                <span className="value-pill">{selectedDeviceLabel(deviceOptions, selectedDevice)}</span>
              </SettingRow>
              <SettingRow title="Input" description="Text insertion strategy">
                <span className="value-pill">
                  {inputMethodLabels[settings.inputMethod] || settings.inputMethod}
                </span>
              </SettingRow>
              <SettingRow title="Auth" description="Authentication status">
                <span className="value-pill">{renderAuthStatus}</span>
              </SettingRow>
              <SettingRow title="Target" description="Binary target triple">
                <span className="value-pill">{buildInfo?.target || "Unknown"}</span>
              </SettingRow>
            </SettingsGroup>
          </motion.div>
        )}
      </section>
    </main>
  );
}

function SettingsGroup({ title, description, children }) {
  return (
    <section className="settings-group">
      <div className="settings-group-heading">
        <h2>{title}</h2>
        {description && <p>{description}</p>}
      </div>
      <div className="settings-group-body">{children}</div>
    </section>
  );
}

function OnboardingLoading({ appIcon }) {
  return (
    <main className="onboarding-shell">
      <section className="onboarding-card onboarding-loading">
        <div className="sidebar-brand onboarding-brand">
          <div className="brand-mark">{appIcon ? <img src={appIcon} alt="Dou Voice" /> : "DV"}</div>
          <div>
            <strong>Dou Voice</strong>
            <span>Preparing setup</span>
          </div>
        </div>
      </section>
    </main>
  );
}

function OnboardingWizard({
  appIcon,
  auth,
  authPath,
  devices,
  draft,
  displayedHotkey,
  capturingHotkey,
  step,
  onStepChange,
  onOpenLogin,
  onExportAuth,
  onCheckAuth,
  onDraftChange,
  onHotkeyCapture,
  onDefaultHotkey,
  onTestRecording,
  onFinish,
}) {
  const authReady = Boolean(auth?.loadOk);
  const selectedDevice = draft.selectedInputDevice || "default";
  const steps = [
    { title: "Doubao Session", detail: authStatusLabel(auth) },
    { title: "Input Basics", detail: selectedDeviceLabel(devices, selectedDevice) },
    { title: "Press To Talk", detail: displayedHotkey },
    { title: "Ready Check", detail: authReady ? "Ready" : "Auth required" },
  ];
  const active = Math.max(0, Math.min(step, steps.length - 1));

  return (
    <main className="onboarding-shell">
      <section className="onboarding-card">
        <header className="onboarding-head">
          <div className="sidebar-brand onboarding-brand">
            <div className="brand-mark">
              {appIcon ? <img src={appIcon} alt="Dou Voice" /> : "DV"}
            </div>
            <div>
              <strong>Dou Voice</strong>
              <span>First run setup</span>
            </div>
          </div>
          <div className="onboarding-progress" aria-label="Setup progress">
            {steps.map((item, index) => (
              <button
                key={item.title}
                type="button"
                data-active={index === active}
                data-done={index < active}
                onClick={() => onStepChange(index)}
              >
                <span>{index + 1}</span>
                <strong>{item.title}</strong>
              </button>
            ))}
          </div>
        </header>

        <div className="onboarding-body">
          {active === 0 && (
            <OnboardingPage
              title="Connect Doubao"
              description="Sign in once, then export the local session used by speech recognition."
            >
              <div className="onboarding-status-grid">
                <RuntimeLine label="Status" value={authStatusLabel(auth)} tone={authReady ? "ready" : "warning"} />
                <RuntimeLine label="Auth File" value={authPath || "Resolving path..."} />
              </div>
              <div className="onboarding-actions">
                <button className="primary" type="button" onClick={onOpenLogin}>
                  Open Login
                </button>
                <button type="button" onClick={onExportAuth}>
                  Export Auth
                </button>
                <button type="button" onClick={onCheckAuth}>
                  Refresh
                </button>
              </div>
            </OnboardingPage>
          )}

          {active === 1 && (
            <OnboardingPage
              title="Choose input behavior"
              description="Keep defaults unless the wrong microphone or insertion method is selected."
            >
              <div className="onboarding-fields">
                <Field label="Microphone" htmlFor="onboarding-input-device">
                  <select
                    id="onboarding-input-device"
                    value={selectedDevice}
                    onChange={(event) =>
                      onDraftChange({
                        selectedInputDevice:
                          event.target.value === "default" ? null : event.target.value,
                      })
                    }
                  >
                    {devices.map((device) => (
                      <option key={device.id} value={device.id}>
                        {device.name}
                        {device.isDefault && device.id !== "default" ? " (system default)" : ""}
                      </option>
                    ))}
                  </select>
                </Field>
                <Field label="Text insertion" htmlFor="onboarding-input-method">
                  <select
                    id="onboarding-input-method"
                    value={draft.inputMethod}
                    onChange={(event) => onDraftChange({ inputMethod: event.target.value })}
                  >
                    <option value="direct">Direct typing with fallback</option>
                    <option value="clipboardPaste">Clipboard paste</option>
                  </select>
                </Field>
                <div className="onboarding-toggle-row">
                  <Toggle
                    checked={draft.overlayEnabled !== false}
                    title="Overlay"
                    subtitle=""
                    onChange={(checked) => onDraftChange({ overlayEnabled: checked })}
                  />
                  <Toggle
                    checked={draft.soundEnabled !== false}
                    title="Sound"
                    subtitle=""
                    onChange={(checked) => onDraftChange({ soundEnabled: checked })}
                  />
                </div>
              </div>
            </OnboardingPage>
          )}

          {active === 2 && (
            <OnboardingPage
              title="Set the press-to-talk hotkey"
              description="The default is shared across platforms. Change it only if it conflicts with your workflow."
            >
              <div className="onboarding-hotkey">
                <input readOnly spellCheck="false" value={capturingHotkey ? displayedHotkey : draft.hotkey} />
                <button type="button" onClick={onHotkeyCapture} disabled={capturingHotkey}>
                  Change
                </button>
                <button type="button" onClick={onDefaultHotkey}>
                  Default
                </button>
              </div>
            </OnboardingPage>
          )}

          {active === 3 && (
            <OnboardingPage
              title="Run a short check"
              description="Save the setup, then use the test recording once before relying on the global hotkey."
            >
              <div className="onboarding-status-grid">
                <RuntimeLine label="Auth" value={authStatusLabel(auth)} tone={authReady ? "ready" : "warning"} />
                <RuntimeLine label="Microphone" value={selectedDeviceLabel(devices, selectedDevice)} />
                <RuntimeLine label="Hotkey" value={draft.hotkey} />
                <RuntimeLine label="Insertion" value={inputMethodLabels[draft.inputMethod] || draft.inputMethod} />
              </div>
              <div className="onboarding-actions">
                <button type="button" onClick={onTestRecording} disabled={!authReady}>
                  Test Recording
                </button>
              </div>
            </OnboardingPage>
          )}
        </div>

        <footer className="onboarding-foot">
          <button type="button" onClick={() => onStepChange(active - 1)} disabled={active === 0}>
            Back
          </button>
          <button
            className={active === steps.length - 1 ? "primary" : ""}
            type="button"
            onClick={active === steps.length - 1 ? onFinish : () => onStepChange(active + 1)}
            disabled={active === steps.length - 1 && !authReady}
          >
            {active === steps.length - 1 ? "Finish Setup" : "Next"}
          </button>
        </footer>
      </section>
    </main>
  );
}

function OnboardingPage({ title, description, children }) {
  return (
    <section className="onboarding-page">
      <div>
        <span className="eyebrow">Setup</span>
        <h1>{title}</h1>
        <p>{description}</p>
      </div>
      {children}
    </section>
  );
}

function RuntimeLine({ label, value, tone = "neutral" }) {
  return (
    <div className="runtime-line" data-tone={tone}>
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  );
}

function SettingRow({ title, description, layout = "horizontal", children }) {
  return (
    <div className="setting-row" data-layout={layout}>
      <div className="setting-copy">
        <h3>{title}</h3>
        <p>{description}</p>
      </div>
      <div className="setting-control">{children}</div>
    </div>
  );
}

function StatusPill({ phase }) {
  return (
    <div className="status-pill" data-phase={phase || "idle"}>
      <span className="status-dot" />
      <span>{phaseLabels[phase] || phase || "Idle"}</span>
    </div>
  );
}

function Metric({ label, value }) {
  return (
    <div>
      <dt>{label}</dt>
      <dd>{value}</dd>
    </div>
  );
}

function Field({ label, htmlFor, className = "", children }) {
  return (
    <div className={`field-stack ${className}`.trim()}>
      <label htmlFor={htmlFor}>{label}</label>
      {children}
    </div>
  );
}

function Toggle({ checked, title, subtitle, onChange }) {
  return (
    <label className="toggle-field">
      <input type="checkbox" checked={checked} onChange={(event) => onChange(event.target.checked)} />
      <span>
        <strong>{title}</strong>
        <small>{subtitle}</small>
      </span>
    </label>
  );
}

async function runAction(action, writeLog) {
  try {
    await action();
  } catch (error) {
    writeLog(String(error));
  }
}

function defaultSettings() {
  return {
    hotkey: DEFAULT_HOTKEY,
    inputMethod: "direct",
    selectedInputDevice: null,
    soundEnabled: true,
    overlayEnabled: true,
  };
}

function normalizeSettings(settings = {}) {
  return {
    hotkey: settings.hotkey || DEFAULT_HOTKEY,
    inputMethod: settings.inputMethod || "direct",
    selectedInputDevice: settings.selectedInputDevice || null,
    soundEnabled: settings.soundEnabled !== false,
    overlayEnabled: settings.overlayEnabled !== false,
  };
}

function normalizeVoiceStatus(status = {}) {
  return {
    phase: status.phase || "idle",
    message: status.message || "Ready.",
    lastText: status.lastText || "",
  };
}

function normalizeDevices(items = []) {
  const devices = items.length ? items : [{ id: "default", name: "Default", isDefault: true }];
  return devices.map((device) => ({
    id: device.id || device.name || "default",
    name: device.name || device.id || "Unknown",
    isDefault: Boolean(device.isDefault),
  }));
}

function withSelectedDevice(devices, selected) {
  const out = devices.length ? [...devices] : [{ id: "default", name: "Default", isDefault: true }];
  if (selected && !out.some((device) => device.id === selected)) {
    out.push({ id: selected, name: `${selected} (unavailable)`, isDefault: false });
  }
  return out;
}

function selectedDeviceLabel(devices, selected) {
  const device = devices.find((item) => item.id === selected);
  return device?.name || "Default";
}

function authStatusLabel(auth) {
  if (!auth) return "Unknown";
  if (auth.loadOk) return `Available (${auth.cookieCount || 0} cookies)`;
  return auth.exists ? "Invalid" : "Missing";
}

function formatCommit(info) {
  if (!info?.commitShortHash || info.commitShortHash === "unknown") return "Unknown";
  return `${info.commitShortHash}${info.gitDirty ? " (dirty)" : ""}`;
}

function formatBuildTime(value) {
  const ms = Number(value);
  if (!Number.isFinite(ms) || ms <= 0) return "Unknown";
  return new Date(ms).toLocaleString();
}

function blockEvent(event) {
  event.preventDefault();
  event.stopPropagation();
}

function isModifierKey(key) {
  return MODIFIER_KEYS.includes(key);
}

function formatHotkeyEvent(event, { onCancel }) {
  const mainKey = displayKeyForEvent(event);
  if (
    mainKey === "Escape" &&
    !event.ctrlKey &&
    !event.altKey &&
    !event.shiftKey &&
    !event.metaKey
  ) {
    onCancel();
    return null;
  }

  const parts = [];
  if (event.ctrlKey) parts.push("Ctrl");
  if (event.altKey) parts.push("Alt");
  if (event.shiftKey) parts.push("Shift");
  if (event.metaKey) parts.push(META_KEY_LABEL);

  const modifierOnly = isModifierKey(event.key);
  if (!modifierOnly && mainKey) parts.push(mainKey);

  const hasMainKey = !modifierOnly && Boolean(mainKey);
  if ((hasMainKey && parts.length >= 2) || (!hasMainKey && parts.length >= 2)) {
    return [...new Set(parts)].join("+");
  }
  return null;
}

function displayKeyForEvent(event) {
  if (/^Key[A-Z]$/.test(event.code)) return event.code.slice(3);
  if (/^Digit[0-9]$/.test(event.code)) return event.code.slice(5);
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(event.code)) return event.code;

  const keyMap = {
    " ": "Space",
    Spacebar: "Space",
    ArrowDown: "ArrowDown",
    ArrowLeft: "ArrowLeft",
    ArrowRight: "ArrowRight",
    ArrowUp: "ArrowUp",
    Backquote: "Backquote",
    Backslash: "Backslash",
    Backspace: "Backspace",
    BracketLeft: "BracketLeft",
    BracketRight: "BracketRight",
    Comma: "Comma",
    Delete: "Delete",
    End: "End",
    Equal: "Equal",
    Enter: "Enter",
    Home: "Home",
    Insert: "Insert",
    Minus: "Minus",
    PageDown: "PageDown",
    PageUp: "PageUp",
    Period: "Period",
    Quote: "Quote",
    Semicolon: "Semicolon",
    Slash: "Slash",
    Tab: "Tab",
    Escape: "Escape",
  };
  return keyMap[event.code] || keyMap[event.key] || null;
}

const rootElement = document.querySelector("#root");
if (!rootElement) {
  document.body.textContent = "Dou Voice failed to initialize: missing root element.";
  throw new Error("missing #root element");
}

createRoot(rootElement).render(<App />);
