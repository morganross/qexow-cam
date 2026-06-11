import { main as camMain } from "./cli.js";

export async function main(args) {
  if (args.length < 2) {
    throw new Error('usage: codex-send <agent-name> "message" [--from <agent>]');
  }
  const forwarded = ["send", ...args];
  if (process.env.CAM_SOURCE_AGENT && !args.includes("--from")) {
    forwarded.push("--from", process.env.CAM_SOURCE_AGENT);
  }
  await camMain(forwarded);
}
