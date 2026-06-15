const fs = require('node:fs/promises');
const path = require('node:path');

const rootDir = path.resolve(__dirname, '..', '..');
const isWindows = process.platform === 'win32';
const source = isWindows
  ? path.join(rootDir, 'target', 'release', 'qexow-cam.exe')
  : path.join(rootDir, 'target', 'release', 'qexow-cam');
const targetDir = path.join(rootDir, 'gui', 'resources', 'bin');
const target = path.join(targetDir, isWindows ? 'cam.exe' : 'cam');

async function main() {
  await fs.access(source);
  await fs.mkdir(targetDir, { recursive: true });
  await fs.copyFile(source, target);
  if (!isWindows) {
    await fs.chmod(target, 0o755);
  }
  console.log(`bundled CAM binary: ${target}`);
}

main().catch((error) => {
  console.error(`failed to prepare CAM binary: ${error.message}`);
  process.exit(1);
});
