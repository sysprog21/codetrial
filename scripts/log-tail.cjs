"use strict";
// What a growing log file has gained since it was last read. A reader that
// polls a log every second cannot afford to read all of it each time: by the
// end of a 90 minute interview that is a megabyte per poll.

const fs = require("fs");

/// A reader of `file` that returns, each time it is called, the complete lines
/// the file gained since the last call, or "" when it gained none. A line
/// still being written is kept back until its newline arrives, so no caller
/// sees half of one and nothing is returned twice. A file that is not there,
/// or that went away between being found and being read, reads as empty. One
/// that was replaced, which shows as another inode or as fewer bytes than were
/// already read, is read from its start.
function createLogTail(file) {
  let inode = null;
  let offset = 0;
  let rest = Buffer.alloc(0);
  const restart = (ino) => {
    inode = ino;
    offset = 0;
    rest = Buffer.alloc(0);
  };
  return () => {
    let stat;
    try {
      stat = fs.statSync(file);
    } catch {
      // Gone: whatever comes back under this name is another file, even if it
      // is given the same inode.
      restart(null);
      return "";
    }
    if (stat.ino !== inode || stat.size < offset) restart(stat.ino);
    if (stat.size > offset) {
      const fresh = Buffer.alloc(stat.size - offset);
      let read;
      try {
        const fd = fs.openSync(file, "r");
        try {
          read = fs.readSync(fd, fresh, 0, fresh.length, offset);
        } finally {
          fs.closeSync(fd);
        }
      } catch {
        return "";
      }
      offset += read;
      rest = Buffer.concat([rest, fresh.subarray(0, read)]);
    }
    const cut = rest.lastIndexOf(10);
    const lines = rest.subarray(0, cut + 1).toString("utf8");
    rest = rest.subarray(cut + 1);
    return lines;
  };
}

module.exports = { createLogTail };
