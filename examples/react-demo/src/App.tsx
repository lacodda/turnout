import { useState } from "react";

// Handed over by `turnout dev` (or read from .env.development.local by Vite);
// the project itself never names a port.
const gateway = import.meta.env.VITE_API_URL as string | undefined;

function jar() {
  return document.cookie || "(none - the jar keeps them)";
}

export function App() {
  const [result, setResult] = useState("Nothing asked yet.");
  const [cookies, setCookies] = useState(jar);

  async function call(path: string) {
    if (!gateway) {
      setResult("No gateway: run this app with `turnout dev` or set VITE_API_URL.");
      return;
    }
    setResult(`GET ${gateway}${path} ...`);
    try {
      const response = await fetch(`${gateway}${path}`);
      const text = await response.text();
      setResult(`${response.status} ${response.statusText}\n${text}`);
    } catch (error) {
      setResult(`Failed: ${error instanceof Error ? error.message : String(error)}`);
    }
    setCookies(jar());
  }

  return (
    <main>
      <p className="eyebrow">turnout react demo</p>
      <h1>One app, one address, any stand behind it</h1>
      <dl>
        <dt>Served at</dt>
        <dd>{window.location.origin}</dd>
        <dt>Gateway</dt>
        <dd>{gateway ?? "not set - run with `turnout dev`"}</dd>
        <dt>document.cookie</dt>
        <dd>{cookies}</dd>
      </dl>
      <div className="actions">
        <button onClick={() => call("/get?from=react-demo")}>Ping the stand</button>
        <button onClick={() => call("/cookies/set?demo=react")}>Set a cookie on the stand</button>
        <button onClick={() => call("/cookies")}>Read cookies</button>
      </div>
      <pre>{result}</pre>
      <p className="hint">
        The stand's cookie never reaches this browser: the gateway keeps it in a jar for this app and
        stand, and sends it back on the next request. `turnout use react-demo OTHER` switches both the
        stand and the jar - no reload, no env edits.
      </p>
    </main>
  );
}
