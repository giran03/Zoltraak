import { useEffect, useState } from "react";
import { version as bundledAppVersion } from "../../package.json";
import { engine, hasEngine } from "../lib/engine";

export function AppVersion() {
  const [version, setVersion] = useState(bundledAppVersion);

  useEffect(() => {
    if (!hasEngine()) return;
    let active = true;
    void engine.getAppVersion().then(value => {
      if (active && value) setVersion(value);
    }).catch(() => {});
    return () => { active = false; };
  }, []);

  return (
    <span aria-label={`App version ${version}`} className="describer shrink-0 whitespace-nowrap text-[11px] text-ink-dim">
      v{version}
    </span>
  );
}
