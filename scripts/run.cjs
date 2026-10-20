const { spawn } = require('node:child_process');
const path = require('node:path');
const os = require('node:os');
const fs = require('node:fs');
const [tool, ...args] = process.argv.slice(2);
const cargoBin = path.join(os.homedir(), '.cargo', 'bin');
const env = { ...process.env, PATH: cargoBin + path.delimiter + process.env.PATH };
let executable;
let toolArgs;
if (tool === 'tauri') {
  executable = process.execPath;
  toolArgs = [path.join(__dirname, '..', 'node_modules', '@tauri-apps', 'cli', 'tauri.js'), ...args];
} else if (tool === 'cargo') {
  const localCargo = path.join(cargoBin, process.platform === 'win32' ? 'cargo.exe' : 'cargo');
  executable = fs.existsSync(localCargo) ? localCargo : 'cargo';
  toolArgs = args;
} else { throw new Error('Supported tools: tauri, cargo'); }
const child = spawn(executable, toolArgs, { cwd: path.join(__dirname, '..'), env, stdio: 'inherit', windowsHide: true });
child.on('error', (error) => { console.error(error.message); process.exitCode = 1; });
child.on('exit', (code) => { process.exitCode = code ?? 1; });
