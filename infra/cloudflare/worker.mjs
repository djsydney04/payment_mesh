const fallback = `<!doctype html>
<html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Return to Mesh</title><main><h1>Return to Mesh</h1>
<p>Open Mesh to continue connecting your test bank. If sign-in was interrupted, start Connect bank again in the app.</p>
</main></html>`;

export default {
  async fetch(request, env) {
    const headers = {
      "Cache-Control": "no-store",
      "Referrer-Policy": "no-referrer",
      "X-Content-Type-Options": "nosniff",
      "Content-Security-Policy": "default-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'",
    };
    if (request.method !== "GET" && request.method !== "HEAD") {
      return new Response("Method not allowed", { status: 405, headers: { ...headers, Allow: "GET, HEAD" } });
    }
    const path = new URL(request.url).pathname;
    let body;
    let status = 200;
    if (path === "/.well-known/apple-app-site-association") {
      headers["Content-Type"] = "application/json";
      body = env.AASA_JSON;
      if (!body) {
        body = "Association configuration unavailable";
        headers["Content-Type"] = "text/plain; charset=utf-8";
        status = 503;
      }
    } else if (path === "/plaid/") {
      headers["Content-Type"] = "text/html; charset=utf-8";
      body = fallback;
    } else {
      headers["Content-Type"] = "text/plain; charset=utf-8";
      body = "Not found";
      status = 404;
    }
    return new Response(request.method === "HEAD" ? null : body, { status, headers });
  },
};
