// Optional presentation preferences must never prevent the app from starting.
export const safeStorage = {
  getItem(key: string): string | null { try { return localStorage.getItem(key); } catch { return null; } },
  setItem(key: string, value: string): void { try { localStorage.setItem(key, value); } catch { /* Preferences are optional. */ } },
  removeItem(key: string): void { try { localStorage.removeItem(key); } catch { /* Preferences are optional. */ } },
};
