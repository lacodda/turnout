// One-off: rasterize turnout brand SVGs into PNGs + a multi-size .ico.
// Run from the docs package so it resolves the local sharp install:
//   node export-assets.mjs
import sharp from "sharp";
import fs from "node:fs";
import path from "node:path";

const ASSETS = "C:/Projects/turnout/assets";

// Which level of the mark goes into which size is decided by `levelFor`
// below, never by habit. This script used to take the S tile for every size,
// and a 256px icon of a filled hexagon is a coloured blob in the taskbar,
// not the mark - the owner saw it on nitid first, the whole line had it.
const S = path.join(ASSETS, "logo-s.svg");
const M = path.join(ASSETS, "logo-m.svg");
const L = path.join(ASSETS, "logo.svg");
const BANNER = path.join(ASSETS, "banner.svg");

// The level rule of the line: S at 27px and below (only the filled tile
// survives there), M from 28 to 63, L from 64 up. Applied per image inside
// the .ico, not per file. Held by a test in tests/release_consistency.rs.
function levelFor(size) {
  if (size <= 27) return S;
  if (size <= 63) return M;
  return L;
}

// Largest first. Windows picks by closest size and ignores order, but
// readers that take the first entry verbatim exist - kilna's titlebar was
// stretched from a 16px entry for exactly this reason.
const ICO_SIZES = [256, 128, 64, 48, 32, 24, 16];

async function png(src, size, out) {
  await sharp(src, { density: 384 }).resize(size, size).png().toFile(out);
}

// Minimal ICO container: header + directory entries + embedded PNG payloads.
function buildIco(pngBuffers, sizes) {
  const count = pngBuffers.length;
  const header = Buffer.alloc(6);
  header.writeUInt16LE(0, 0); // reserved
  header.writeUInt16LE(1, 2); // type: icon
  header.writeUInt16LE(count, 4);

  const entries = Buffer.alloc(16 * count);
  let offset = 6 + 16 * count;
  pngBuffers.forEach((buf, i) => {
    const size = sizes[i];
    const e = 16 * i;
    entries.writeUInt8(size >= 256 ? 0 : size, e + 0); // width (0 means 256)
    entries.writeUInt8(size >= 256 ? 0 : size, e + 1); // height
    entries.writeUInt8(0, e + 2); // palette
    entries.writeUInt8(0, e + 3); // reserved
    entries.writeUInt16LE(1, e + 4); // color planes
    entries.writeUInt16LE(32, e + 6); // bits per pixel
    entries.writeUInt32LE(buf.length, e + 8);
    entries.writeUInt32LE(offset, e + 12);
    offset += buf.length;
  });

  return Buffer.concat([header, entries, ...pngBuffers]);
}

const icoParts = [];
for (const size of ICO_SIZES) {
  icoParts.push(await sharp(levelFor(size), { density: 384 }).resize(size, size).png().toBuffer());
}
fs.writeFileSync(path.join(ASSETS, "icon.ico"), buildIco(icoParts, ICO_SIZES));
console.log("wrote icon.ico");

// Favicon + docs logo. The favicon is the one documented exception to
// `levelFor`: a browser draws it into 16px of tab whatever size the file is,
// and the outline does not survive that - the canon names it explicitly.
await png(S, 32, path.join(ASSETS, "favicon-32.png"));
await png(levelFor(180), 180, path.join(ASSETS, "apple-touch-icon.png"));
await png(levelFor(512), 512, path.join(ASSETS, "logo-512.png"));
console.log("wrote pngs");

// GitHub social preview: 1280x640. Two adjustments to the banner: its plate
// spans the full 720px while the artwork only fills the left ~570px (trim the
// tail, or it lands off-centre), and the rounded plate over an identical
// background leaves a visible seam (drop it and keep the inner rows only).
const bannerWidth = 1600;
const bannerHeight = Math.round((bannerWidth * 170) / 720);
const inset = Math.round((bannerWidth * 6) / 720); // clears the plate's rounded edge
const banner = await sharp(BANNER, { density: 384 })
  .resize({ width: bannerWidth })
  .extract({ left: inset, top: inset, width: 1290 - inset, height: bannerHeight - 2 * inset })
  .png()
  .toBuffer();

await sharp({
  create: { width: 1280, height: 640, channels: 4, background: "#1B2126" },
})
  .composite([{ input: await sharp(banner).resize({ width: 880 }).png().toBuffer(), gravity: "centre" }])
  .png()
  .toFile(path.join(ASSETS, "social-preview.png"));
console.log("wrote social-preview.png");
