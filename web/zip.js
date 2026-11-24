// A store-only ZIP writer: the files go in as they are, with no compression and
// no dependency, in the format every unzip reads. Names are plain ASCII, so no
// tool has to guess an encoding, and nothing larger than 4 GiB (no ZIP64) is
// accepted.

const encoder = new TextEncoder();
const LIMIT = 0xffffffff;
// Printable ASCII, and no leading slash or `..`: a name that stays where it is.
const PLAIN_NAME = /^(?![/])(?!.*\.\.)[\x20-\x7e]+$/;

const TABLE = new Uint32Array(256).map((_, index) => {
  let value = index;
  for (let bit = 0; bit < 8; bit += 1)
    value = value & 1 ? 0xedb88320 ^ (value >>> 1) : value >>> 1;
  return value >>> 0;
});

/// The CRC-32 of `bytes`, as ZIP and gzip take it.
export function crc32(bytes) {
  let value = 0xffffffff;
  for (const byte of bytes)
    value = TABLE[(value ^ byte) & 0xff] ^ (value >>> 8);
  return (value ^ 0xffffffff) >>> 0;
}

/// The DOS time and date fields of `when`, the only clock a ZIP header has.
function dosTime(when) {
  const date = new Date(when);
  const year = Math.min(Math.max(date.getFullYear(), 1980), 2107);
  return {
    time:
      (date.getHours() << 11) |
      (date.getMinutes() << 5) |
      (date.getSeconds() >> 1),
    date: ((year - 1980) << 9) | ((date.getMonth() + 1) << 5) | date.getDate(),
  };
}

/// One ZIP holding `files`, each `{name, bytes}`, stamped with the time `when`.
export function zipStore(files, when = Date.now()) {
  const { time, date } = dosTime(when);
  const locals = [];
  const centrals = [];
  let offset = 0;
  for (const { name, bytes } of files) {
    if (!PLAIN_NAME.test(name))
      throw new Error(`"${name}" is not a plain ASCII file name`);
    const label = encoder.encode(name);
    const crc = crc32(bytes);
    const local = new DataView(new ArrayBuffer(30));
    local.setUint32(0, 0x04034b50, true);
    local.setUint16(4, 20, true); // version needed
    local.setUint16(6, 0, true);
    local.setUint16(8, 0, true); // stored
    local.setUint16(10, time, true);
    local.setUint16(12, date, true);
    local.setUint32(14, crc, true);
    local.setUint32(18, bytes.length, true);
    local.setUint32(22, bytes.length, true);
    local.setUint16(26, label.length, true);
    const central = new DataView(new ArrayBuffer(46));
    central.setUint32(0, 0x02014b50, true);
    central.setUint16(4, 20, true); // version made by
    central.setUint16(6, 20, true);
    central.setUint16(8, 0, true);
    central.setUint16(10, 0, true);
    central.setUint16(12, time, true);
    central.setUint16(14, date, true);
    central.setUint32(16, crc, true);
    central.setUint32(20, bytes.length, true);
    central.setUint32(24, bytes.length, true);
    central.setUint16(28, label.length, true);
    central.setUint32(42, offset, true);
    locals.push(new Uint8Array(local.buffer), label, bytes);
    centrals.push(new Uint8Array(central.buffer), label);
    offset += 30 + label.length + bytes.length;
    if (offset > LIMIT) throw new Error("the files are too large for a ZIP");
  }
  const size = centrals.reduce((sum, part) => sum + part.length, 0);
  const end = new DataView(new ArrayBuffer(22));
  end.setUint32(0, 0x06054b50, true);
  end.setUint16(8, files.length, true);
  end.setUint16(10, files.length, true);
  end.setUint32(12, size, true);
  end.setUint32(16, offset, true);
  const parts = [...locals, ...centrals, new Uint8Array(end.buffer)];
  const out = new Uint8Array(parts.reduce((sum, part) => sum + part.length, 0));
  let at = 0;
  for (const part of parts) {
    out.set(part, at);
    at += part.length;
  }
  return out;
}
