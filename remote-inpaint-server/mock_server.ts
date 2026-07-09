// Local mock of the remote inpaint server, for testing koharu's
// `remote-inpaint` engine without renting anything:
//   bun run mock_server.ts
// It echoes the uploaded MASK back as the "inpainted" result — if the page's
// Inpainted layer turns into the black-and-white mask, the whole client path
// (config, multipart, auth, decode, scene write) works.
const KEY = process.env.API_KEY ?? "";

Bun.serve({
  port: 8787,
  async fetch(req) {
    const url = new URL(req.url);
    if (KEY && req.headers.get("authorization") !== `Bearer ${KEY}`) {
      return new Response("unauthorized", { status: 401 });
    }
    if (url.pathname === "/health") {
      return Response.json({ status: "ok", model: "mock-echo-mask", ready: true });
    }
    if (url.pathname === "/inpaint" && req.method === "POST") {
      const form = await req.formData();
      const mask = form.get("mask");
      if (!(mask instanceof File)) return new Response("missing mask part", { status: 400 });
      console.log(
        `inpaint: image=${(form.get("image") as File | null)?.size ?? "?"}B ` +
          `mask=${mask.size}B prompt=${JSON.stringify(form.get("prompt"))}`,
      );
      return new Response(await mask.arrayBuffer(), {
        headers: { "content-type": "image/png" },
      });
    }
    return new Response("not found", { status: 404 });
  },
});

console.log(`mock remote-inpaint server on http://127.0.0.1:8787 (auth: ${KEY ? "on" : "off"})`);
