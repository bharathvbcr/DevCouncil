import { spawnSync } from "node:child_process";

const projectRoot = process.env.DEVCOUNCIL_PROJECT_ROOT || process.cwd();

function runHook(event, payload) {
  const args = ["hook", event, "--client", "opencode", "--project-root", projectRoot];
  const result = spawnSync("devcouncil", args, {
    input: JSON.stringify(payload ?? {}),
    encoding: "utf-8",
    env: { ...process.env, DEVCOUNCIL_PROJECT_ROOT: projectRoot },
  });
  if (result.status === 2) {
    throw new Error(result.stderr || result.stdout || "DevCouncil blocked the tool call.");
  }
}

export const DevCouncilOpenCodeHook = async () => ({
  "tool.execute.before": async (input, output) => {
    runHook("pre-tool-use", { tool: input.tool, arguments: output.args });
  },
  "tool.execute.after": async (input, output) => {
    runHook("post-tool-use", { tool: input.tool, arguments: output.args });
  },
});
