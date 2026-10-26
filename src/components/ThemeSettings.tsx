import { useLayoutEffect, useState } from "react";

type ThemePreference = "system" | "light" | "dark";
const STORAGE_KEY = "minutus.theme";

function savedPreference(): ThemePreference {
  try {
    const saved = localStorage.getItem(STORAGE_KEY);
    if (saved === "light" || saved === "dark" || saved === "system") return saved;
  } catch {
    // Light appearance remains usable when browser storage is unavailable.
  }
  return "light";
}

export function useThemePreference() {
  const [preference, setPreference] = useState<ThemePreference>(savedPreference);
  const [saveError, setSaveError] = useState(false);

  useLayoutEffect(() => {
    const system = window.matchMedia("(prefers-color-scheme: dark)");
    const apply = () => {
      document.documentElement.dataset.theme = preference === "system"
        ? (system.matches ? "dark" : "light")
        : preference;
    };
    apply();
    system.addEventListener("change", apply);
    return () => system.removeEventListener("change", apply);
  }, [preference]);

  function changePreference(value: ThemePreference) {
    setPreference(value);
    try {
      localStorage.setItem(STORAGE_KEY, value);
      setSaveError(false);
    } catch {
      setSaveError(true);
    }
  }

  return { preference, changePreference, saveError };
}

export function ThemeSettings({ preference, changePreference, saveError }: ReturnType<typeof useThemePreference>) {
  return (
    <section className="app-settings-section" aria-labelledby="theme-settings-title">
      <div className="app-settings-section-heading">
        <div><p className="eyebrow">Appearance</p><h2 id="theme-settings-title">Theme</h2></div>
      </div>
      <div className="appearance-setting">
        <label htmlFor="theme-select">Application appearance</label>
        <select id="theme-select" value={preference} onChange={(event) => changePreference(event.target.value as ThemePreference)}>
          <option value="light">Light</option>
          <option value="dark">Dark</option>
          <option value="system">System</option>
        </select>
      </div>
      {saveError ? <p className="download-error" role="alert">Your theme changed, but could not be saved for next time.</p> : null}
    </section>
  );
}
