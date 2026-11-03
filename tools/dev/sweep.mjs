// Before `npm run dev` and `npm run build`: on Windows, the developer's own sweep (tools/dev/sweep.ps1, not in the
// repository), run as it always was. Elsewhere there's nothing to sweep: macOS lets a running WestPlay's files be
// replaced, which Windows doesn't.
import { execFileSync } from "node:child_process";

if (process.platform === "win32") {
  try {
    execFileSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "tools/dev/sweep.ps1"], {
      stdio: "inherit",
    });
  } catch (error) {
    process.exit(error.status ?? 1);
  }
}
