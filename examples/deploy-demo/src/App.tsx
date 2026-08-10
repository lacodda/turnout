/**
 * One page whose only job is to prove a deploy landed: it shows when the
 * bundle was built, so a stale page is obvious at a glance.
 */
export function App() {
  const built = __BUILD_STAMP__;
  return (
    <main>
      <p className="eyebrow">turnout deploy demo</p>
      <h1>This page was built at</h1>
      <p className="stamp">{built}</p>
      <p className="hint">
        Rebuild and deploy again - if the stamp does not change, the upload did not land.
      </p>
    </main>
  );
}
