import { homedir } from "node:os";
import { join } from "node:path";
import { readFileSync } from "node:fs";
//#region src/index.ts
const inject = ["webServer"];
function apply(ctx) {
	ctx.effect(() => ctx.webServer.register({
		kind: "exact",
		path: "/dsh-desktop-shell/status",
		handler: (_req, res) => {
			const home = process.env.DSH_HOME || join(homedir(), ".dsh");
			const profile = join(home, "profiles", "web", "package.json");
			let manifest = {};
			try {
				manifest = JSON.parse(readFileSync(profile, "utf8"));
			} catch {}
			res.writeHead(200, {
				"content-type": "application/json; charset=utf-8",
				"cache-control": "no-store"
			});
			res.end(JSON.stringify({
				profile: "web",
				dshHome: home,
				bundles: manifest?.dsh?.profile?.bundles ?? [],
				dependencies: manifest?.dependencies ?? {}
			}));
		}
	}), "dsh-desktop-shell: status route");
}
//#endregion
export { apply, inject };
