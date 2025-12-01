import fs = require("fs");

/**
 * Capture console logs and write them to app.log
 */
export const initFileLogger = () => {
  const logFile = fs.createWriteStream("app.log", { flags: "a" });
  const logStdout = process.stdout;
  const getColorCode = (level: string) => {
    switch (level) {
      case "error":
        return "\x1b[31m";
      case "warn":
        return "\x1b[33m";
      case "info":
        return "\x1b[34m";
      default:
        return "\x1b[0m";
    }
  };

  const createLogger =
    (level: string) =>
    (...args: any[]) => {
      const date = Date.now();
      const content = JSON.stringify(args.length === 1 ? args[0] : [...args]);

      logFile.write(`[${date}][${level[0]?.toUpperCase()}] - ${content}\n`, (err) => {
        if (err) throw err;
      });
      logStdout.write(`${date} - ${getColorCode(level)}${content}\x1b[0m\n`);
    };
  console.log = createLogger("log");
  console.error = createLogger("error");
  console.warn = createLogger("warn");
  console.info = createLogger("info");
};

export const readLogfile = async (lines = 80) => {
  const fs = require("fs");
  const readline = require("readline");
  const stream = require("stream");

  const readLastLines = async (filePath: string, numLines: number) => {
    const instream = fs.createReadStream(filePath);
    const outstream = new stream();
    const rl = readline.createInterface(instream, outstream);

    const lines: string[] = [];
    for await (const line of rl) {
      lines.push(line);
      if (lines.length > numLines) {
        lines.shift();
      }
    }
    return lines.join("\n");
  };
  return await readLastLines("app.log", lines);
};

export const logFileChangeWatcher = (change: () => void) => {
  fs.watch("app.log", (eventType, _filename) => {
    if (eventType === "change") {
      change();
    }
  });
};
