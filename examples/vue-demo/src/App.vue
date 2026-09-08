<script setup lang="ts">
import { ref } from "vue";

// Handed over by `turnout dev` (or read from .env.development.local by Vite);
// the project itself never names a port.
const gateway = import.meta.env.VITE_API_URL as string | undefined;
const servedAt = window.location.origin;
const result = ref("Nothing asked yet.");
const cookies = ref(document.cookie || "(none - the jar keeps them)");

async function call(path: string) {
  if (!gateway) {
    result.value = "No gateway: run this app with `turnout dev` or set VITE_API_URL.";
    return;
  }
  result.value = `GET ${gateway}${path} ...`;
  try {
    const response = await fetch(`${gateway}${path}`);
    const text = await response.text();
    result.value = `${response.status} ${response.statusText}\n${text}`;
  } catch (error) {
    result.value = `Failed: ${error instanceof Error ? error.message : String(error)}`;
  }
  cookies.value = document.cookie || "(none - the jar keeps them)";
}
</script>

<template>
  <main>
    <p class="eyebrow">turnout vue demo</p>
    <h1>One app, one address, any stand behind it</h1>
    <dl>
      <dt>Served at</dt>
      <dd>{{ servedAt }}</dd>
      <dt>Gateway</dt>
      <dd>{{ gateway ?? "not set - run with `turnout dev`" }}</dd>
      <dt>document.cookie</dt>
      <dd>{{ cookies }}</dd>
    </dl>
    <div class="actions">
      <button @click="call('/get?from=vue-demo')">Ping the stand</button>
      <button @click="call('/cookies/set?demo=vue')">Set a cookie on the stand</button>
      <button @click="call('/cookies')">Read cookies</button>
    </div>
    <pre>{{ result }}</pre>
    <p class="hint">
      The stand's cookie never reaches this browser: the gateway keeps it in a jar for this
      app and stand, and sends it back on the next request. `turnout use vue-demo OTHER`
      switches both the stand and the jar - no reload, no env edits.
    </p>
  </main>
</template>
