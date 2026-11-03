import { useCallback } from "react";
import { bootstrapAppearance, useAppearance } from "./useAppearance";

export type ThemePreference = "system" | "light" | "dark";

export function bootstrapTheme() {
  bootstrapAppearance();
}

export function useTheme() {
  const { settings, setThemeMode } = useAppearance();

  const preference: ThemePreference =
    settings.themeMode === "light" || settings.themeMode === "dark" || settings.themeMode === "system"
      ? settings.themeMode
      : "dark";

  const set = useCallback(
    (next: ThemePreference) => {
      setThemeMode(next);
    },
    [setThemeMode],
  );

  return { preference, set };
}
