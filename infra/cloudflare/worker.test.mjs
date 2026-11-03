import assert from "node:assert/strict";
import worker from "./worker.mjs";

// Only synthetic identifiers; no external services or account mutations.
export async function verifyWorker() {
  const aasa = { applinks: { details: [{ appIDs: ["TESTTEAM01.app.mesh.sandbox"], components: [{ "/": "/plaid/" }] }] } };
  const env = { AASA_JSON: JSON.stringify(aasa) };
  const request = (path, method = "GET") => worker.fetch(new Request(`https://payment-mesh.example${path}`, { method }), env);
  const association = await request("/.well-known/apple-app-site-association");
  assert.equal(association.status, 200);
  assert.equal(association.headers.get("Content-Type"), "application/json");
  assert.equal(association.headers.get("Location"), null);
  assert.equal(await association.text(), JSON.stringify(aasa));
  const callback = await request("/plaid/?oauth_state_id=private-marker&redirect=https://evil.example");
  assert.equal(callback.status, 200);
  assert.equal(callback.headers.get("Cache-Control"), "no-store");
  assert.equal(callback.headers.get("Referrer-Policy"), "no-referrer");
  assert.equal(callback.headers.get("Location"), null);
  const html = await callback.text();
  assert.ok(html.includes("Return to Mesh"));
  assert.ok(!html.includes("private-marker") && !html.includes("evil.example"));
  assert.equal(await (await request("/plaid/", "HEAD")).text(), "");
  assert.equal((await request("/v1/funding")).status, 404);
  assert.equal((await request("/plaid/", "POST")).status, 405);
  assert.equal((await worker.fetch(new Request("https://payment-mesh.example/.well-known/apple-app-site-association"), {})).status, 503);
}

await verifyWorker();
console.log("OAuth Worker request checks passed.");
