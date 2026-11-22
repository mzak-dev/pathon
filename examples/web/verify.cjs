// Opens the built web example in Chromium and checks what M0-06, M0-09, M0-10 and M1-01 ask for on the web:
// the device is created asynchronously, frames run, the canvas shows the clear colour (sRGB-encoded exactly like
// native) and the logged `Caps` agree with what the browser's own adapter reports.
//
//   node verify.cjs <dir with index.html and pkg/>      (needs the `playwright` package and a Chromium)
//
// PATHON_CHROME_ARGS adds browser flags (CI: software WebGPU), PATHON_CHROME_CHANNEL or PATHON_CHROME_EXECUTABLE pick
// the browser.
// Exit codes: 0 all good, 1 a check failed, 2 the browser has no WebGPU adapter (environment, not a bug).

const fs = require("node:fs");
const http = require("node:http");
const path = require("node:path");
const { chromium } = require("playwright");

const EXPECTED_PIXEL = [89, 124, 160]; // clear colour (0.1, 0.2, 0.35) linear, sRGB-encoded
const PIXEL_TOLERANCE = 2;
const FRAME_TIMEOUT_MS = 30_000;

// WebGPU feature name -> pathon `Features` name. SUBGROUP is left out on purpose: wgpu 30 has no web mapping for
// `subgroups`, so Caps cannot report it there.
const FEATURES = {
  "timestamp-query": "TIMESTAMP_QUERY",
  "indirect-first-instance": "INDIRECT_FIRST_INSTANCE",
  "shader-f16": "SHADER_F16",
  "float32-filterable": "FLOAT32_FILTERABLE",
  "texture-compression-bc": "TEXTURE_COMPRESSION_BC",
  "texture-compression-etc2": "TEXTURE_COMPRESSION_ETC2",
  "texture-compression-astc": "TEXTURE_COMPRESSION_ASTC",
  "depth-clip-control": "DEPTH_CLIP_CONTROL",
  "depth32float-stencil8": "DEPTH32FLOAT_STENCIL8",
};

const LIMITS = {
  maxTextureDimension2D: "max_texture_dimension_2d",
  maxTextureArrayLayers: "max_texture_array_layers",
  maxBindGroups: "max_bind_groups",
  maxStorageBufferBindingSize: "max_storage_buffer_binding_size",
  maxBufferSize: "max_buffer_size",
  maxColorAttachments: "max_color_attachments",
};

const MIME = { ".html": "text/html", ".js": "text/javascript", ".wasm": "application/wasm" };

function serve(root) {
  const server = http.createServer((request, response) => {
    const name = request.url.split("?")[0] === "/" ? "/index.html" : request.url.split("?")[0];
    const file = path.join(root, path.normalize(name));
    if (!file.startsWith(root) || !fs.existsSync(file)) {
      response.writeHead(404).end();
      return;
    }
    response.writeHead(200, { "content-type": MIME[path.extname(file)] ?? "application/octet-stream" });
    fs.createReadStream(file).pipe(response);
  });
  return new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve(server)));
}

function parseCaps(line) {
  const featureText = /features: Features\(([^)]*)\)/.exec(line)?.[1] ?? "";
  const features = new Set(featureText.split("|").map((name) => name.trim()).filter((name) => name && name !== "0x0"));
  const limits = {};
  for (const [, name, value] of line.matchAll(/(max_[a-z0-9_]+): (\d+)/g)) {
    limits[name] = Number(value);
  }
  const backend = /backend: (\w+)/.exec(line)?.[1];
  return { backend, features, limits };
}

function pixelAfterFrame(page) {
  return page.evaluate(
    () =>
      new Promise((resolve) =>
        requestAnimationFrame(() => {
          const copy = document.createElement("canvas");
          copy.width = 4;
          copy.height = 4;
          const context = copy.getContext("2d");
          context.drawImage(document.querySelector("canvas"), 0, 0, 4, 4);
          resolve([...context.getImageData(1, 1, 1, 1).data]);
        }),
      ),
  );
}

async function main() {
  const root = path.resolve(process.argv[2] ?? ".");
  const server = await serve(root);
  const args = (process.env.PATHON_CHROME_ARGS ?? "").split(/\s+/).filter(Boolean);
  const browser = await chromium.launch({
    channel: process.env.PATHON_CHROME_CHANNEL || undefined,
    executablePath: process.env.PATHON_CHROME_EXECUTABLE || undefined,
    args,
  });
  const failures = [];
  const summary = [];
  try {
    const page = await browser.newPage();
    const logs = [];
    page.on("console", (message) => logs.push(message.text()));
    page.on("pageerror", (error) => logs.push(`pageerror: ${error.message}`));
    await page.goto(`http://127.0.0.1:${server.address().port}/`);

    const adapter = await page.evaluate(async () => {
      const adapter = navigator.gpu ? await navigator.gpu.requestAdapter() : null;
      if (!adapter) return null;
      const limits = {};
      for (const key in adapter.limits) limits[key] = adapter.limits[key];
      return { features: [...adapter.features], limits, userAgent: navigator.userAgent, info: adapter.info?.description ?? "" };
    });
    if (!adapter) {
      console.error("no WebGPU adapter in this browser: nothing to verify");
      process.exitCode = 2;
      return;
    }

    const deadline = Date.now() + FRAME_TIMEOUT_MS;
    while (!logs.some((line) => line.includes("frame 0")) && Date.now() < deadline) {
      await page.waitForTimeout(200);
    }
    const capsLine = logs.find((line) => line.includes("caps: Caps {"));
    if (!capsLine) {
      failures.push(`the example never logged its Caps (console: ${logs.join(" | ")})`);
    } else {
      const caps = parseCaps(capsLine);
      if (caps.backend !== "WebGpu") failures.push(`backend is ${caps.backend}, expected WebGpu`);
      for (const [webName, ourName] of Object.entries(FEATURES)) {
        const browserHas = adapter.features.includes(webName);
        if (browserHas !== caps.features.has(ourName)) {
          failures.push(`feature ${ourName}: browser ${browserHas}, Caps ${caps.features.has(ourName)}`);
        }
      }
      for (const [webName, ourName] of Object.entries(LIMITS)) {
        if (adapter.limits[webName] !== caps.limits[ourName]) {
          failures.push(`limit ${ourName}: browser ${adapter.limits[webName]}, Caps ${caps.limits[ourName]}`);
        }
      }
      summary.push(`- features in Caps: \`${[...caps.features].join(" | ") || "none"}\``);
      summary.push(`- browser adapter features: \`${adapter.features.join(", ")}\``);
      summary.push(`- limits compared: ${Object.keys(LIMITS).map((name) => `${name}=${adapter.limits[name]}`).join(", ")}`);
    }

    await page.waitForTimeout(500);
    const pixel = await pixelAfterFrame(page);
    const off = pixel.slice(0, 3).map((value, index) => Math.abs(value - EXPECTED_PIXEL[index]));
    if (off.some((delta) => delta > PIXEL_TOLERANCE)) {
      failures.push(`canvas pixel is ${pixel.slice(0, 3)}, expected ${EXPECTED_PIXEL} (the clear colour as native shows it)`);
    }
    const frameLogs = logs.filter((line) => /: frame \d+$/.test(line));
    if (frameLogs.length === 0) failures.push("no frame was logged: the render loop did not start");
    summary.unshift(
      `- browser: ${adapter.userAgent}${adapter.info ? ` (${adapter.info})` : ""}`,
      `- canvas pixel after a frame: ${pixel.slice(0, 3)} (expected ${EXPECTED_PIXEL})`,
      `- frame log lines: ${frameLogs.length}`,
    );
    const errors = logs.filter((line) => /error|panick/i.test(line) && !line.includes("favicon"));
    if (errors.length > 0) failures.push(`console errors: ${errors.join(" | ")}`);
  } finally {
    await browser.close();
    server.close();
  }
  const report = [`### Web example in Chromium: ${failures.length === 0 ? "ok" : "FAILED"}`, ...summary, ...failures.map((failure) => `- FAILED: ${failure}`)].join("\n");
  console.log(report);
  if (process.env.GITHUB_STEP_SUMMARY) fs.appendFileSync(process.env.GITHUB_STEP_SUMMARY, `${report}\n`);
  if (failures.length > 0) process.exitCode = 1;
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
