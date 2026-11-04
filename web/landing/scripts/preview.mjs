import http from "node:http";
import { readFile, stat } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";
const root = path.resolve(fileURLToPath(new URL("../dist/", import.meta.url)));
const types = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json",
  ".svg": "image/svg+xml",
  ".webp": "image/webp",
  ".gif": "image/gif",
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".mp4": "video/mp4",
  ".ttf": "font/ttf",
  ".txt": "text/plain",
  ".xml": "application/xml",
};
const server = http.createServer(async (req, res) => {
  try {
    const url = new URL(req.url, "http://localhost");
    let file = path.resolve(root, "." + decodeURIComponent(url.pathname));
    if (file !== root && !file.startsWith(root + path.sep)) {
      res.writeHead(403);
      res.end();
      return;
    }
    if ((await stat(file)).isDirectory()) file = path.join(file, "index.html");
    const data = await readFile(file);
    res.setHeader(
      "Content-Type",
      types[path.extname(file)] || "application/octet-stream",
    );
    res.setHeader("Cache-Control", "no-store");
    const range = req.headers.range;
    if (range) {
      const m = /^bytes=(\d+)-(\d*)$/.exec(range);
      if (m) {
        const start = +m[1],
          end = Math.min(m[2] ? +m[2] : data.length - 1, data.length - 1);
        if (start > end) {
          res.writeHead(416);
          res.end();
          return;
        }
        res.writeHead(206, {
          "Content-Range": `bytes ${start}-${end}/${data.length}`,
          "Content-Length": end - start + 1,
          "Accept-Ranges": "bytes",
        });
        res.end(data.subarray(start, end + 1));
        return;
      }
    }
    res.writeHead(200, {
      "Content-Length": data.length,
      "Accept-Ranges": "bytes",
    });
    res.end(data);
  } catch {
    res.writeHead(404, { "Content-Type": "text/plain" });
    res.end("Not found");
  }
});
server.listen(Number(process.env.PORT || 4173), "127.0.0.1", () =>
  console.log(`Local: http://127.0.0.1:${server.address().port}`),
);
