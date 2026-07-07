#!/usr/bin/env node
/**
 * Registry validator — the CI gate for vendor packages.
 *
 * Mirrors the structural rules in @kerf/core (validateManifest /
 * validatePlaybook) as a zero-dependency script so it runs before any
 * install step. The two rules that matter most:
 *   - canaries never spend (budget_minor must be literal 0);
 *   - money-adjacent playbook steps must carry assertions and may not
 *     use agent_repair.
 */
import { readdirSync, readFileSync, existsSync, statSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const registryDir = join(dirname(fileURLToPath(import.meta.url)), "..", "packages", "registry");
const problems = [];

function check(cond, msg) {
  if (!cond) problems.push(msg);
}

const vendors = readdirSync(registryDir).filter((d) =>
  statSync(join(registryDir, d)).isDirectory(),
);
check(vendors.length > 0, "registry is empty");

for (const vendor of vendors) {
  const base = join(registryDir, vendor);
  const manifestPath = join(base, "manifest.json");
  if (!existsSync(manifestPath)) {
    problems.push(`${vendor}: missing manifest.json`);
    continue;
  }
  const m = JSON.parse(readFileSync(manifestPath, "utf8"));
  const where = (s) => `${vendor}/manifest.json: ${s}`;

  check(m.format === 1, where(`unknown format ${m.format}`));
  check(m.id === vendor, where(`id "${m.id}" != directory name "${vendor}"`));
  check(Array.isArray(m.domains) && m.domains.length > 0, where("domains empty — the allowlist would be empty"));
  check(m.capabilities && Object.keys(m.capabilities).length > 0, where("no capabilities declared"));
  check(["L0", "L1", "L2", "L3"].includes(m.autonomy_ceiling), where(`bad autonomy_ceiling "${m.autonomy_ceiling}"`));
  if (m.canary) {
    check(m.canary.budget_minor === 0, where("canary budget_minor must be 0 — canaries never spend"));
    check(
      existsSync(join(base, m.canary.playbook ?? "")),
      where(`canary playbook "${m.canary.playbook}" not found`),
    );
  }

  for (const [name, cap] of Object.entries(m.capabilities ?? {})) {
    if (cap.playbook) {
      const pbPath = join(base, cap.playbook);
      if (!existsSync(pbPath)) {
        problems.push(where(`capability "${name}" playbook "${cap.playbook}" not found`));
        continue;
      }
      const pb = JSON.parse(readFileSync(pbPath, "utf8"));
      const pwhere = (s) => `${vendor}/${cap.playbook}: ${s}`;
      check(pb.format === 1, pwhere(`unknown format ${pb.format}`));
      check(pb.vendor === vendor, pwhere(`vendor "${pb.vendor}" != "${vendor}"`));
      check(Array.isArray(pb.steps) && pb.steps.length > 0, pwhere("no steps"));
      const seen = new Set();
      for (const step of pb.steps ?? []) {
        check(!seen.has(step.id), pwhere(`duplicate step id "${step.id}"`));
        seen.add(step.id);
        if (step.money_adjacent) {
          check(
            Array.isArray(step.assert) && step.assert.length > 0,
            pwhere(`step "${step.id}" is money_adjacent but carries no assertions`),
          );
          check(
            step.on_fail !== "agent_repair",
            pwhere(`step "${step.id}": agent_repair is illegal on a money_adjacent step`),
          );
        }
      }
    }
  }
}

if (problems.length) {
  console.error(`registry INVALID (${problems.length} problem${problems.length === 1 ? "" : "s"}):`);
  for (const p of problems) console.error(`  - ${p}`);
  process.exit(1);
}
console.log(`registry OK — ${vendors.length} vendor${vendors.length === 1 ? "" : "s"} validated: ${vendors.join(", ")}`);
