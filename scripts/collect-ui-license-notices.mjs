// Collect installed browser runtime dependency notices from the npm lockfile.
// No local source paths appear in the output. Run npm ci before this script.
import { copyFile, mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const [uiDirectory, outputDirectory] = process.argv.slice(2);
if (!uiDirectory || !outputDirectory) {
  throw new Error("Usage: node collect-ui-license-notices.mjs UI_DIRECTORY OUTPUT_DIRECTORY");
}
const lock = JSON.parse(await readFile(path.join(uiDirectory, "package-lock.json"), "utf8"));
const buildOnly = new Set(["vite", "@vitejs/plugin-react"]);
const selected = new Set();
function visit(relative) {
  if (selected.has(relative)) return;
  selected.add(relative);
  const entry = lock.packages[relative];
  const dependencies = { ...entry.dependencies, ...entry.optionalDependencies, ...entry.peerDependencies };
  for (const dependency of Object.keys(dependencies)) {
    let parent = relative;
    let found;
    while (true) {
      const candidate = `${parent ? parent + "/" : ""}node_modules/${dependency}`;
      if (lock.packages[candidate]) { found = candidate; break; }
      if (!parent) break;
      parent = parent.slice(0, Math.max(0, parent.lastIndexOf("/node_modules/")));
    }
    if (found) visit(found);
    else if (!entry.optionalDependencies?.[dependency] && !entry.peerDependenciesMeta?.[dependency]?.optional) {
      throw new Error(`Missing locked dependency ${dependency} for ${relative}`);
    }
  }
}
for (const dependency of Object.keys(lock.packages[""].dependencies)) {
  if (!buildOnly.has(dependency)) visit(`node_modules/${dependency}`);
}
const fallbackDirectory = fileURLToPath(new URL("../packaging/ui-licenses/", import.meta.url));
const fallbackNotices = {
  "@tauri-apps/api@2.11.1": "tauri-api-2.11.1-MIT.txt",
  "react-remove-scroll-bar@2.3.8": "react-remove-scroll-bar-2.3.8-MIT.txt",
};
const notices = [];
for (const [relative, entry] of Object.entries(lock.packages)) {
  if (!selected.has(relative)) continue;
  const directory = path.join(uiDirectory, relative);
  let manifest;
  try {
    manifest = JSON.parse(await readFile(path.join(directory, "package.json"), "utf8"));
  } catch (error) {
    // Platform-specific optional packages need not be installed on this host.
    if (entry.optional && error.code === "ENOENT") continue;
    throw new Error(`Missing installed package: ${relative}`, { cause: error });
  }
  if (manifest.version !== entry.version) {
    throw new Error(`Installed version differs from package-lock.json: ${manifest.name}`);
  }
  const identity = `${manifest.name}@${manifest.version}`;
  if (notices.some((item) => item.package === identity)) continue;
  const noticeFiles = (await readdir(directory, { withFileTypes: true }))
    .filter((item) => item.isFile() && /^(licen[cs]e|copying|copyright|notice|ofl)([.-]|$)/i.test(item.name))
    .map((item) => item.name);
  const destination = path.join(outputDirectory, identity.replaceAll("/", "__"));
  await mkdir(destination, { recursive: true });
  for (const name of noticeFiles) await copyFile(path.join(directory, name), path.join(destination, name));
  if (!noticeFiles.length && fallbackNotices[identity]) {
    const name = fallbackNotices[identity];
    await copyFile(path.join(fallbackDirectory, name), path.join(destination, name));
    noticeFiles.push(name);
  }
  if (!noticeFiles.length) throw new Error(`No license notice found for ${identity}`);
  notices.push({ package: identity, license: manifest.license || entry.license || "See included notices", files: noticeFiles });
}
notices.sort((a, b) => a.package.localeCompare(b.package));
await mkdir(outputDirectory, { recursive: true });
await writeFile(path.join(outputDirectory, "index.json"), JSON.stringify(notices, null, 2) + "\n");
console.log(`Collected notices for ${notices.length} JavaScript packages.`);
