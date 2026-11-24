// A reader of store-only ZIPs for tests, written apart from web/zip.js so that
// one does not vouch for the other: it checks every field it reads.
import { createHash } from "node:crypto";

function crc32(bytes) {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit += 1)
      crc = crc & 1 ? (crc >>> 1) ^ 0xedb88320 : crc >>> 1;
  }
  return (crc ^ 0xffffffff) >>> 0;
}

/// `[{name, bytes}]` of a ZIP; throws on anything a strict unzip would refuse.
export function readZip(buffer) {
  const data = new DataView(buffer.buffer, buffer.byteOffset, buffer.length);
  const end = buffer.length - 22;
  if (data.getUint32(end, true) !== 0x06054b50)
    throw new Error("no end record");
  const count = data.getUint16(end + 10, true);
  let at = data.getUint32(end + 16, true);
  if (at + data.getUint32(end + 12, true) !== end)
    throw new Error("the central directory does not end at the end record");
  const files = [];
  for (let entry = 0; entry < count; entry += 1) {
    if (data.getUint32(at, true) !== 0x02014b50) throw new Error("bad entry");
    const flags = data.getUint16(at + 8, true);
    const method = data.getUint16(at + 10, true);
    const crc = data.getUint32(at + 16, true);
    const size = data.getUint32(at + 24, true);
    const nameLength = data.getUint16(at + 28, true);
    const local = data.getUint32(at + 42, true);
    const name = new TextDecoder().decode(
      buffer.subarray(at + 46, at + 46 + nameLength),
    );
    if (method !== 0 || flags !== 0) throw new Error("not a plain stored file");
    if (!/^[\x20-\x7e]+$/.test(name)) throw new Error(`${name}: not ASCII`);
    if (data.getUint32(local, true) !== 0x04034b50)
      throw new Error("bad local");
    // The local header says the same as the central entry, as unzip checks.
    if (
      data.getUint16(local + 6, true) !== flags ||
      data.getUint16(local + 8, true) !== method ||
      data.getUint32(local + 14, true) !== crc ||
      data.getUint32(local + 18, true) !== size ||
      data.getUint32(local + 22, true) !== size
    )
      throw new Error(`${name}: the local header differs from the directory`);
    const start = local + 30 + data.getUint16(local + 26, true);
    const bytes = buffer.subarray(start, start + size);
    if (crc32(bytes) !== crc) throw new Error(`${name}: wrong CRC`);
    files.push({ name, bytes });
    at += 46 + nameLength;
  }
  return files;
}

export function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}
