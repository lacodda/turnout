#!/usr/bin/env node
// Thin launcher: forwards everything to the downloaded turnout binary.
const path = require("path");
const { spawnSync } = require("child_process");

const exe = path.join(__dirname, process.platform === "win32" ? "turnout.exe" : "turnout");
if (!require("fs").existsSync(exe)) {
  console.error("turnout-cli: binary missing - reinstall the package (npm i -g turnout-cli)");
  process.exit(1);
}
const result = spawnSync(exe, process.argv.slice(2), { stdio: "inherit" });
process.exit(result.status === null ? 1 : result.status);
