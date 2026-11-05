import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";

// One content version for the entire CSS/module set prevents mixed deployments,
// including cached transitive imports such as app.js -> site-core.js.
export async function versionAssets(directory, files, pages) {
  const content = await Promise.all(
    files.map((file) => readFile(path.join(directory, file), "utf8")),
  );
  const version = createHash("sha256")
    .update(JSON.stringify(files.map((file, i) => [file, content[i]])))
    .digest("hex")
    .slice(0, 16);
  const names = Object.fromEntries(
    files.map((file) => {
      const ext = path.extname(file);
      return [file, `${file.slice(0, -ext.length)}.${version}${ext}`];
    }),
  );
  const rewrite = (text) => {
    for (const [file, versioned] of Object.entries(names)) {
      for (const prefix of ["", "./", "/"]) {
        text = text.replaceAll(`"${prefix}${file}"`, `"${prefix}${versioned}"`);
      }
    }
    return text;
  };
  await Promise.all(
    files.map((file, i) =>
      writeFile(path.join(directory, names[file]), rewrite(content[i])),
    ),
  );
  for (const page of pages) {
    const file = path.join(directory, page);
    await writeFile(file, rewrite(await readFile(file, "utf8")));
  }
  return names;
}
