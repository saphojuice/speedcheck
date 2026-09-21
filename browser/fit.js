// SAPHOJUICE browser fit test v0.3. Measures on this device. Nothing is looked up.
// Every value below is labeled with the API that produced it and wrapped so one failing probe
// never breaks the rest (see the `errors` map on the returned object).
// Never collected, anywhere in this file: IP, geolocation, timezone, canvas/audio/font
// fingerprints, the full navigator.userAgent string, plugins, cookies, full referrer URLs.
(function(){
  // ================================================================== identity
  function getInstallId(){
    try {
      let id = localStorage.getItem('sj_install_id');
      if (!id) { id = (crypto.randomUUID ? crypto.randomUUID() : 'i-' + Math.random().toString(36).slice(2) + Date.now().toString(36)); localStorage.setItem('sj_install_id', id); }
      return id;
    } catch (e) { return null; }
  }

  // ================================================================== hand-assembled wasm module (bandwidth)
  // (module (memory (import "env" "memory") 2048 2048 shared)
  //   (func (export "copy") (param $dst i32) (param $src i32) (param $len i32)
  //     local.get $dst local.get $src local.get $len memory.copy))
  // 2048 pages = 128 MiB; fixed so the module bytes don't depend on the thread count at runtime.
  const WASM_PAGES = 2048;
  const WASM_BYTES = new Uint8Array([
    0x00,0x61,0x73,0x6d,0x01,0x00,0x00,0x00,               // magic, version
    0x01,0x07,0x01,0x60,0x03,0x7f,0x7f,0x7f,0x00,           // type section: (i32,i32,i32)->()
    0x02,0x12,0x01,0x03,0x65,0x6e,0x76,0x06,0x6d,0x65,0x6d,0x6f,0x72,0x79,0x02,0x03,0x80,0x10,0x80,0x10, // import "env"."memory", shared, min=max=2048
    0x03,0x02,0x01,0x00,                                     // function section: fn0 : type0
    0x07,0x08,0x01,0x04,0x63,0x6f,0x70,0x79,0x00,0x00,       // export "copy" func 0
    0x0a,0x0e,0x01,0x0c,0x00,0x20,0x00,0x20,0x01,0x20,0x02,0xfc,0x0a,0x00,0x00,0x0b // code: local.get x3; memory.copy; end
  ]);
  let _wasmModule = null;
  function wasmModule(){ return _wasmModule || (_wasmModule = WebAssembly.compile(WASM_BYTES)); }

  const workerSrc = `
    self.onmessage = async (e) => {
      const { kind, bytes, ms } = e.data;
      if (kind === 'bw') {
        const n = Math.max(1024, bytes >> 3);
        const a = new Float64Array(n), b = new Float64Array(n), c = new Float64Array(n);
        for (let i = 0; i < n; i++) { b[i] = i % 97; c[i] = (i % 89) * 0.5; }
        const s = 1.000001; let reps = 0; const t0 = performance.now();
        while (performance.now() - t0 < ms) { for (let i = 0; i < n; i++) a[i] = b[i] + s * c[i]; reps++; }
        const secs = (performance.now() - t0) / 1000;
        self.postMessage({ bytes: reps * 3 * 8 * n, secs, sink: a[n >> 1] });
      } else if (kind === 'wasmbw') {
        try {
          const { module, memory, offset, half } = e.data;
          const inst = await WebAssembly.instantiate(module, { env: { memory } });
          const copy = inst.exports.copy;
          const src = offset, dst = offset + half;
          let reps = 0; const t0 = performance.now();
          while (performance.now() - t0 < ms) { copy(dst, src, half); reps++; }
          const secs = (performance.now() - t0) / 1000;
          self.postMessage({ ok: true, bytes: reps * 2 * half, secs });
        } catch (err) { self.postMessage({ ok: false, error: String(err) }); }
      } else if (kind === 'opfs') {
        try {
          const size = e.data.size || (64 * 1024 * 1024);
          const buf = new Uint8Array(size);
          for (let i = 0; i < size; i += 4096) buf[i] = i & 0xff;
          const root = await navigator.storage.getDirectory();
          const fh = await root.getFileHandle('sj_opfs_test.bin', { create: true });
          const access = await fh.createSyncAccessHandle();
          const t0 = performance.now();
          access.write(buf, { at: 0 });
          access.flush();
          const wsecs = (performance.now() - t0) / 1000;
          const rbuf = new Uint8Array(size);
          const t1 = performance.now();
          access.read(rbuf, { at: 0 });
          const rsecs = (performance.now() - t1) / 1000;
          access.close();
          await root.removeEntry('sj_opfs_test.bin');
          self.postMessage({ ok: true, write_gbps: size / wsecs / 1e9, read_gbps: size / rsecs / 1e9 });
        } catch (err) { self.postMessage({ ok: false, error: String(err) }); }
      } else {
        const N = 256; const A = new Float32Array(N*N), B = new Float32Array(N*N), C = new Float32Array(N*N);
        for (let i = 0; i < N*N; i++) { A[i] = (i % 13) * 0.01; B[i] = (i % 7) * 0.02; }
        let best = 0; const t0 = performance.now();
        while (performance.now() - t0 < ms) {
          const t1 = performance.now();
          for (let i = 0; i < N; i++) { const ai = i*N; for (let j = 0; j < N; j++) C[ai+j] = 0;
            for (let k = 0; k < N; k++) { const aik = A[ai+k], bk = k*N; for (let j = 0; j < N; j++) C[ai+j] += aik * B[bk+j]; } }
          const gf = 2 * N*N*N / ((performance.now() - t1) / 1000) / 1e9; if (gf > best) best = gf;
        }
        self.postMessage({ gflops: best, sink: C[0] });
      }
    };`;
  function spawn(){ return new Worker(URL.createObjectURL(new Blob([workerSrc], {type:'text/javascript'}))); }
  function runAll(kind, bytes, ms, threads, extra){
    return Promise.all(Array.from({length: threads}, () => new Promise(res => {
      const w = spawn(); w.onmessage = e => { res(e.data); w.terminate(); }; w.postMessage({ kind, bytes, ms, ...extra });
    })));
  }

  // ================================================================== cpu bandwidth: wasm threads, JS floor fallback
  async function wasmBandwidth(threads, ms){
    if (typeof SharedArrayBuffer === 'undefined' || !self.crossOriginIsolated) return null;
    let module, memory;
    try {
      module = await wasmModule();
      memory = new WebAssembly.Memory({ initial: WASM_PAGES, maximum: WASM_PAGES, shared: true });
    } catch (e) { return null; }
    const totalBytes = WASM_PAGES * 65536;
    const perThread = Math.floor(totalBytes / threads / 2) * 2; // even, so the src/dst halves land on distinct bytes
    const half = perThread >> 1;
    const jobs = Array.from({length: threads}, (_, i) => new Promise(res => {
      const w = spawn(); w.onmessage = e => { res(e.data); w.terminate(); };
      w.postMessage({ kind: 'wasmbw', module, memory, offset: i * perThread, half, ms });
    }));
    const r = await Promise.all(jobs);
    if (r.some(x => !x.ok)) return null;
    const total = r.reduce((s, x) => s + x.bytes, 0), maxs = Math.max(...r.map(x => x.secs));
    return maxs > 0 ? total / maxs / 1e9 : null;
  }
  async function jsFloorBandwidth(threads, ms){
    const per = Math.floor(96 * 1024 * 1024 / 3 / threads);
    const r = await runAll('bw', per, ms, threads);
    const total = r.reduce((s, x) => s + x.bytes, 0), maxs = Math.max(...r.map(x => x.secs));
    return total / maxs / 1e9;
  }
  async function cpuBandwidth(ms){
    const threads = Math.max(1, Math.min(16, navigator.hardwareConcurrency || 4));
    const counts = [...new Set([1, Math.ceil(threads / 2), threads])];
    // try wasm at the smallest thread count first; if it isn't available at all, don't bother retrying per count
    let source = 'wasm_threads', bw = 0, bwBy = {};
    const probe = await wasmBandwidth(counts[0], Math.min(300, ms));
    if (probe === null) {
      source = 'js_floor';
      for (const c of counts) { const g = await jsFloorBandwidth(c, ms); bwBy[c] = g; if (g > bw) bw = g; }
    } else {
      bwBy[counts[0]] = probe; bw = probe;
      for (const c of counts.slice(1)) { const g = await wasmBandwidth(c, ms); if (g !== null) { bwBy[c] = g; if (g > bw) bw = g; } }
    }
    return { bw, bwBy, source, threads };
  }

  // cache-cliff sweep: single-thread JS triad at fixed working-set sizes, to see where bandwidth drops
  // as the working set falls out of L1/L2/L3 into DRAM. Diagnostic only (not used for grading), so a
  // plain JS loop is honest here: it is not the headline number, just a shape.
  const CACHE_CLIFF_SIZES = [256*1024, 1536*1024, 6*1024*1024, 24*1024*1024, 96*1024*1024, 384*1024*1024];
  async function cacheCliffSweep(msPerSize){
    const out = [];
    for (const size of CACHE_CLIFF_SIZES) {
      try {
        const r = await runAll('bw', Math.floor(size / 3), msPerSize, 1);
        out.push({ size_bytes: size, gbps: r[0].bytes / r[0].secs / 1e9 });
      } catch (e) { out.push({ size_bytes: size, error: String(e) }); }
    }
    return out;
  }

  // optional 30s sustained pass: bandwidth in 5s windows, to see thermal/power decay. Not run by default
  // (the site is a 20-second test); callers opt in explicitly via measure(onPhase, {sustained:true}).
  async function sustainedRun(threads){
    const windows = [];
    for (let i = 0; i < 6; i++) {
      const g = await wasmBandwidth(threads, 5000).catch(() => null);
      const v = g === null ? await jsFloorBandwidth(threads, 5000) : g;
      windows.push(v);
    }
    const peak = Math.max(...windows), last = windows[windows.length - 1];
    return { series_gbps: windows, window_s: 5, decay_fraction: peak > 0 ? Math.max(0, 1 - last / peak) : 0 };
  }

  // ================================================================== compute (f32 GFLOPS, unchanged method)
  async function compute(threads, ms){
    const r = await runAll('mm', 0, ms, threads);
    return r.reduce((s, x) => s + x.gflops, 0);
  }

  // ================================================================== wasm capability flags
  function wasmValidate(bytes){ try { return WebAssembly.validate(new Uint8Array(bytes)); } catch (e) { return false; } }
  function wasmFeatures(){
    const simd = wasmValidate([0x00,0x61,0x73,0x6d,0x01,0x00,0x00,0x00,0x01,0x05,0x01,0x60,0x00,0x01,0x7b,0x03,0x02,0x01,0x00,0x0a,0x0a,0x01,0x08,0x00,0x41,0x00,0xfd,0x0f,0xfd,0x62,0x0b]);
    const threads = wasmValidate([0x00,0x61,0x73,0x6d,0x01,0x00,0x00,0x00,0x05,0x04,0x01,0x03,0x01,0x01]);
    const bulkMemory = wasmValidate([0x00,0x61,0x73,0x6d,0x01,0x00,0x00,0x00,0x01,0x04,0x01,0x60,0x00,0x00,0x03,0x02,0x01,0x00,0x05,0x03,0x01,0x00,0x01,0x0a,0x0e,0x01,0x0c,0x00,0x41,0x00,0x41,0x00,0x41,0x00,0xfc,0x0a,0x00,0x00,0x0b]);
    // relaxed-simd: i8x16.relaxed_swizzle on two v128 constants. Best-effort byte-level probe.
    const relaxedSimd = wasmValidate([0x00,0x61,0x73,0x6d,0x01,0x00,0x00,0x00,0x01,0x05,0x01,0x60,0x00,0x01,0x7b,0x03,0x02,0x01,0x00,0x0a,0x2a,0x01,0x28,0x00,
      0xfd,0x0c,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0, 0xfd,0x0c,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0, 0xfd,0x80,0x02, 0x0b]);
    const exceptions = typeof WebAssembly.Tag === 'function'; // JS-API proxy for the exception-handling proposal
    return { simd, relaxed_simd: relaxedSimd, threads, bulk_memory: bulkMemory, exceptions, source: 'WebAssembly.validate() / WebAssembly.Tag' };
  }

  // ================================================================== UA client hints, performance.memory
  async function uaHints(){
    if (!(navigator.userAgentData && navigator.userAgentData.getHighEntropyValues)) return { available: false };
    try {
      const v = await navigator.userAgentData.getHighEntropyValues(['architecture','bitness','model','platform','platformVersion','fullVersionList','wow64','formFactors']);
      return { available: true, source: 'navigator.userAgentData.getHighEntropyValues', architecture: v.architecture || '', bitness: v.bitness || '', model: v.model || '', platform: v.platform || '', platformVersion: v.platformVersion || '', wow64: !!v.wow64, formFactors: v.formFactors || [], fullVersionList: (v.fullVersionList || []).map(b => ({ brand: b.brand, version: b.version })) };
    } catch (e) { return { available: false, error: String(e) }; }
  }
  function perfMemory(){
    if (!performance.memory) return { available: false };
    const m = performance.memory;
    return { available: true, source: 'performance.memory', jsHeapSizeLimit: m.jsHeapSizeLimit, totalJSHeapSize: m.totalJSHeapSize, usedJSHeapSize: m.usedJSHeapSize };
  }

  // ================================================================== GPU: WebGL
  function webglInfo(){
    try {
      const canvas = document.createElement('canvas');
      const gl2 = canvas.getContext('webgl2');
      const gl = gl2 || canvas.getContext('webgl') || canvas.getContext('experimental-webgl');
      if (!gl) return { available: false };
      const dbg = gl.getExtension('WEBGL_debug_renderer_info');
      const vendor = dbg ? gl.getParameter(dbg.UNMASKED_VENDOR_WEBGL) : gl.getParameter(gl.VENDOR);
      const renderer = dbg ? gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.RENDERER);
      return {
        available: true, source: 'WebGL RENDERING_CONTEXT + WEBGL_debug_renderer_info',
        webgl2: !!gl2,
        unmasked_vendor: String(vendor || ''), unmasked_renderer: String(renderer || ''),
        gl_version: String(gl.getParameter(gl.VERSION) || ''), glsl_version: String(gl.getParameter(gl.SHADING_LANGUAGE_VERSION) || ''),
        max_texture_size: gl.getParameter(gl.MAX_TEXTURE_SIZE),
        extensions: gl.getSupportedExtensions() || [],
      };
    } catch (e) { return { available: false, error: String(e) }; }
  }

  // ================================================================== GPU: WebGPU (bandwidth + full limits/features)
  const WGSL_BW = `
    @group(0) @binding(0) var<storage, read_write> buf: array<vec4<f32>>;
    @compute @workgroup_size(256)
    fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
      buf[gid.x] = buf[gid.x] + vec4<f32>(1.0, 1.0, 1.0, 1.0);
    }`;
  const WEBGPU_LIMIT_NAMES = ['maxTextureDimension1D','maxTextureDimension2D','maxTextureDimension3D','maxTextureArrayLayers','maxBindGroups','maxBindGroupsPlusVertexBuffers','maxBindingsPerBindGroup','maxDynamicUniformBuffersPerPipelineLayout','maxDynamicStorageBuffersPerPipelineLayout','maxSampledTexturesPerShaderStage','maxSamplersPerShaderStage','maxStorageBuffersPerShaderStage','maxStorageTexturesPerShaderStage','maxUniformBuffersPerShaderStage','maxUniformBufferBindingSize','maxStorageBufferBindingSize','minUniformBufferOffsetAlignment','minStorageBufferOffsetAlignment','maxVertexBuffers','maxBufferSize','maxVertexAttributes','maxVertexBufferArrayStride','maxInterStageShaderVariables','maxColorAttachments','maxColorAttachmentBytesPerSample','maxComputeWorkgroupStorageSize','maxComputeInvocationsPerWorkgroup','maxComputeWorkgroupSizeX','maxComputeWorkgroupSizeY','maxComputeWorkgroupSizeZ','maxComputeWorkgroupsPerDimension'];
  async function gpu(){
    if (!navigator.gpu) return { available: false };
    let adapter, device;
    try {
      adapter = await navigator.gpu.requestAdapter({ powerPreference: 'high-performance' });
      if (!adapter) return { available: false };
      const wantFeatures = ['timestamp-query', 'shader-f16'].filter(f => adapter.features && adapter.features.has(f));
      device = await adapter.requestDevice(wantFeatures.length ? { requiredFeatures: wantFeatures } : {});
    } catch (e) { return { available: false, error: String(e) }; }
    const info = adapter.info || {};
    const limits = {};
    for (const name of WEBGPU_LIMIT_NAMES) { try { if (adapter.limits && name in adapter.limits) limits[name] = adapter.limits[name]; } catch (e) {} }
    const features = adapter.features ? [...adapter.features] : [];
    const base = { available: true, vendor: info.vendor || '', architecture: info.architecture || '', device: info.device || '', description: info.description || '', limits, features, shader_f16: features.includes('shader-f16'), timestamp_query: features.includes('timestamp-query') };
    try {
      const size = 128 * 1024 * 1024; // 128 MB storage buffer, read and written in place
      const buf = device.createBuffer({ size, usage: GPUBufferUsage.STORAGE });
      const mod = device.createShaderModule({ code: WGSL_BW });
      const pipeline = device.createComputePipeline({ layout: 'auto', compute: { module: mod, entryPoint: 'main' } });
      const bindGroup = device.createBindGroup({ layout: pipeline.getBindGroupLayout(0), entries: [{ binding: 0, resource: { buffer: buf } }] });
      const workgroups = Math.ceil((size / 16) / 256);
      const dispatch = enc => { const p = enc.beginComputePass(); p.setPipeline(pipeline); p.setBindGroup(0, bindGroup); p.dispatchWorkgroups(workgroups); p.end(); };
      // warm-up: first dispatch pays pipeline/shader compile cost, exclude it from the timed passes
      { const enc = device.createCommandEncoder(); dispatch(enc); device.queue.submit([enc.finish()]); await device.queue.onSubmittedWorkDone(); }

      const hasTimestamp = device.features.has('timestamp-query');
      let gbps, method, passes;
      if (hasTimestamp) {
        const querySet = device.createQuerySet({ type: 'timestamp', count: 2 });
        const resolveBuf = device.createBuffer({ size: 16, usage: GPUBufferUsage.QUERY_RESOLVE | GPUBufferUsage.COPY_SRC });
        const readBuf = device.createBuffer({ size: 16, usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.MAP_READ });
        passes = 8;
        const enc = device.createCommandEncoder();
        const p = enc.beginComputePass({ timestampWrites: { querySet, beginningOfPassWriteIndex: 0, endOfPassWriteIndex: 1 } });
        p.setPipeline(pipeline); p.setBindGroup(0, bindGroup);
        for (let i = 0; i < passes; i++) p.dispatchWorkgroups(workgroups);
        p.end();
        enc.resolveQuerySet(querySet, 0, 2, resolveBuf, 0);
        enc.copyBufferToBuffer(resolveBuf, 0, readBuf, 0, 16);
        device.queue.submit([enc.finish()]);
        await readBuf.mapAsync(GPUMapMode.READ);
        const ts = new BigUint64Array(readBuf.getMappedRange().slice(0));
        readBuf.unmap();
        const ns = Number(ts[1] - ts[0]);
        gbps = ns > 0 ? (passes * size * 2) / (ns / 1e9) / 1e9 : 0;
        method = 'webgpu_timestamp';
      } else {
        const t0 = performance.now(); passes = 0;
        while (performance.now() - t0 < 200) {
          const enc = device.createCommandEncoder(); dispatch(enc);
          device.queue.submit([enc.finish()]); await device.queue.onSubmittedWorkDone();
          passes++;
        }
        const secs = (performance.now() - t0) / 1000;
        gbps = secs > 0 ? (passes * size * 2) / secs / 1e9 : 0;
        method = 'webgpu_walltime';
      }
      return { ...base, gbps, method, passes };
    } catch (e) { return { ...base, error: String(e) }; }
  }

  // ================================================================== WebNN
  async function webnnInfo(){
    if (!(navigator.ml && navigator.ml.createContext)) return { available: false };
    const out = { available: true, source: 'navigator.ml.createContext' };
    for (const deviceType of ['npu', 'gpu', 'cpu']) {
      try { await navigator.ml.createContext({ deviceType }); out[deviceType] = true; } catch (e) { out[deviceType] = false; }
    }
    return out;
  }

  // ================================================================== MediaCapabilities
  async function mediaCapsInfo(){
    if (!navigator.mediaCapabilities) return { available: false };
    const codecs = {
      h264: 'video/mp4; codecs="avc1.42E01E"',
      vp9: 'video/webm; codecs="vp09.00.10.08"',
      av1: 'video/mp4; codecs="av01.0.04M.08"',
      hevc: 'video/mp4; codecs="hvc1.1.6.L93.B0"',
    };
    const out = { available: true, source: 'navigator.mediaCapabilities.decodingInfo' };
    for (const [name, contentType] of Object.entries(codecs)) {
      try {
        const r = await navigator.mediaCapabilities.decodingInfo({ type: 'file', video: { contentType, width: 1920, height: 1080, bitrate: 2000000, framerate: 30 } });
        out[name] = { supported: r.supported, smooth: r.smooth, power_efficient: r.powerEfficient };
      } catch (e) { out[name] = { error: String(e) }; }
    }
    return out;
  }

  // ================================================================== storage + OPFS + battery + network
  async function storageInfo(){
    const out = {};
    try { if (navigator.storage && navigator.storage.estimate) { const e = await navigator.storage.estimate(); out.estimate = { available: true, source: 'navigator.storage.estimate', quota_bytes: e.quota, usage_bytes: e.usage }; } else out.estimate = { available: false }; }
    catch (e) { out.estimate = { available: false, error: String(e) }; }
    try {
      const r = await runAll('opfs', 0, 0, 1, { size: 64 * 1024 * 1024 });
      out.opfs = r[0].ok ? { available: true, source: 'OPFS createSyncAccessHandle (worker)', write_gbps: r[0].write_gbps, read_gbps: r[0].read_gbps } : { available: false, error: r[0].error };
    } catch (e) { out.opfs = { available: false, error: String(e) }; }
    return out;
  }
  async function batteryInfo(){
    if (!navigator.getBattery) return { available: false };
    try { const b = await navigator.getBattery(); return { available: true, source: 'navigator.getBattery', charging: b.charging, level: b.level, charging_time_s: b.chargingTime, discharging_time_s: b.dischargingTime }; }
    catch (e) { return { available: false, error: String(e) }; }
  }
  function networkInfo(){
    const c = navigator.connection || navigator.mozConnection || navigator.webkitConnection;
    if (!c) return { available: false };
    return { available: true, source: 'navigator.connection', effective_type: c.effectiveType || null, downlink_mbps: typeof c.downlink === 'number' ? c.downlink : null, rtt_ms: typeof c.rtt === 'number' ? c.rtt : null, save_data: !!c.saveData, type: c.type || null };
  }

  // ================================================================== display + form factor
  function measureRefreshRate(){
    return new Promise(resolve => {
      const samples = [];
      let last = null;
      function step(ts){
        if (last !== null) samples.push(ts - last);
        last = ts;
        if (samples.length < 30) requestAnimationFrame(step);
        else {
          const avg = samples.reduce((a, b) => a + b, 0) / samples.length;
          resolve(avg > 0 ? Math.round(1000 / avg) : null);
        }
      }
      requestAnimationFrame(step);
      setTimeout(() => resolve(null), 1500); // safety timeout if rAF stalls
    });
  }
  function mq(query){ try { return matchMedia(query).matches; } catch (e) { return false; } }
  async function displayInfo(){
    return {
      source: 'screen / matchMedia / navigator.maxTouchPoints',
      width: screen.width, height: screen.height, color_depth: screen.colorDepth, dpr: window.devicePixelRatio || 1,
      orientation: (screen.orientation && screen.orientation.type) || null,
      refresh_hz: await measureRefreshRate(),
      hdr: mq('(dynamic-range: high)'), color_gamut_p3: mq('(color-gamut: p3)'), color_gamut_rec2020: mq('(color-gamut: rec2020)'),
      max_touch_points: navigator.maxTouchPoints || 0,
      pointer_fine: mq('(pointer: fine)'), pointer_coarse: mq('(pointer: coarse)'), hover: mq('(hover: hover)'),
    };
  }

  // ================================================================== session
  function utmCampaign(){
    try { return new URLSearchParams(location.search).get('utm_campaign') || null; } catch (e) { return null; }
  }
  function referrerHostname(){
    try { return document.referrer ? new URL(document.referrer).hostname : null; } catch (e) { return null; }
  }
  function sessionInfo(bandwidthSource, gpuAvailable){
    const brands = (navigator.userAgentData && navigator.userAgentData.brands) ? navigator.userAgentData.brands.map(b => ({ brand: b.brand, version: b.version })) : [];
    return {
      source: 'navigator.userAgentData.brands / navigator.languages.length / document.referrer hostname only',
      brands, languages_count: (navigator.languages && navigator.languages.length) || 0,
      cross_origin_isolated: !!self.crossOriginIsolated,
      tier: bandwidthSource === 'wasm_threads' && gpuAvailable ? 'full' : bandwidthSource === 'wasm_threads' ? 'wasm-only' : 'js-floor',
      referrer_hostname: referrerHostname(), utm_campaign: utmCampaign(),
    };
  }

  async function sha256(s){ const d = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(s)); return [...new Uint8Array(d)].map(b => b.toString(16).padStart(2,'0')).join(''); }

  // Physics: tok/s = k * bandwidth / active_bytes; fit = file + kv + reserve <= memory
  // Grades on gpu bandwidth when the model fits the (coarse, browser-reported) gpu/unified memory ceiling,
  // else on cpu bandwidth. Browsers do not expose real VRAM size; navigator.deviceMemory is the best proxy
  // we have, same coarse number Chrome/Edge cap at 8 GB for privacy -- treated here as an estimate, not a spec.
  const RESERVE = 4;
  function grade(m, bwCpu, bwGpu, gpuMemGB, memGB, k, memKnown){
    const need = m.file + m.kv + RESERVE;
    const useGpu = typeof bwGpu === 'number' && bwGpu > 0 && typeof gpuMemGB === 'number' && need <= gpuMemGB;
    const bw = useGpu ? bwGpu : bwCpu;
    const method = useGpu ? 'gpu' : 'cpu';
    const tps = k * bw / m.act;
    const fits = memKnown ? need <= memGB : (need <= memGB ? true : null); // null = unknown above the browser cap
    let band = 'no', label = 'Won’t fit';
    if (fits !== false) { if (tps >= 30) { band='fast'; label='Fast'; } else if (tps >= 15) { band='fast'; label='Feels fast'; } else if (tps >= 8) { band='ok'; label='Reading pace'; } else { band='slow'; label='Slow'; } }
    if (fits === null) label += ', if you have ' + Math.ceil(need) + ' GB';
    return { need, fits, tps, band, label, lo: tps * 0.9, hi: tps * 1.1, bw, method };
  }

  async function measure(onPhase, opts){
    opts = opts || {};
    const t0 = performance.now();
    const errors = {};
    const guard = async (label, fn, fallback) => { try { return await fn(); } catch (e) { errors[label] = String(e); return fallback; } };

    onPhase && onPhase('bandwidth');
    const { bw: cpuBw, bwBy, source: bandwidthSource, threads } = await cpuBandwidth(900);
    const cacheCliff = await guard('cache_cliff', () => cacheCliffSweep(120), []);
    const sustained = opts.sustained ? await guard('sustained', () => sustainedRun(threads), null) : null;

    onPhase && onPhase('compute');
    const gflops = await guard('compute', () => compute(threads, 1500), 0);
    const wasmFeat = await guard('wasm_features', () => Promise.resolve(wasmFeatures()), {});

    onPhase && onPhase('gpu');
    const g = await guard('webgpu', () => gpu(), { available: false });
    const webgl = await guard('webgl', () => Promise.resolve(webglInfo()), { available: false });
    const webnn = await guard('webnn', () => webnnInfo(), { available: false });
    const mediaCaps = await guard('media_capabilities', () => mediaCapsInfo(), { available: false });

    onPhase && onPhase('memory');
    const devMem = navigator.deviceMemory || null;             // capped at 8 by browsers
    const memKnown = false;                                    // browser cannot see free memory
    const memGB = devMem ? devMem : 8;
    const perfMem = await guard('performance_memory', () => Promise.resolve(perfMemory()), { available: false });
    const storage = await guard('storage', () => storageInfo(), {});
    const battery = await guard('battery', () => batteryInfo(), { available: false });
    const network = await guard('network', () => Promise.resolve(networkInfo()), { available: false });

    onPhase && onPhase('display');
    const display = await guard('display', () => displayInfo(), {});
    const ua = await guard('ua_hints', () => uaHints(), { available: false });
    const platform = navigator.userAgentData ? (navigator.userAgentData.platform || '') : navigator.platform || '';
    const archBasic = ua.available ? (ua.architecture || '') : '';
    const uaModel = ua.available ? (ua.model || '') : '';

    const gpuVendor = (g.available && g.vendor) || (webgl.available && webgl.unmasked_vendor) || '';
    const gpuFamily = (g.available && g.architecture) || (webgl.available && webgl.unmasked_renderer) || '';
    const gpuBw = g.available && typeof g.gbps === 'number' ? g.gbps : null;
    const gpuMemGB = g.available ? (devMem || 8) : null; // coarse proxy for VRAM/unified memory; see grade()

    const installId = getInstallId();
    // class_hash: durable hardware-class constants only. No bandwidth (measurement noise would fracture
    // the class), no install_id (that identifies a browser profile, not a chip).
    const cls = await sha256(['v2', platform, archBasic, uaModel, gpuVendor, gpuFamily, threads, devMem || 0].join('|'));

    const session = sessionInfo(bandwidthSource, g.available);
    const ceiling = gpuBw && gpuBw > cpuBw ? gpuBw : cpuBw; // single headline number for callers that want one

    const extra = {
      ua_hints: ua, performance_memory: perfMem, wasm_features: wasmFeat,
      cache_cliff_sweep: cacheCliff, sustained,
      webgl, webgpu: g.available ? { limits: g.limits, features: g.features, method: g.method, passes: g.passes } : { available: false },
      webnn, media_capabilities: mediaCaps,
      storage, battery, network, display, session,
      errors,
    };

    return {
      version: '0.3', install_id: installId, class_hash: cls, source: 'browser', seconds: (performance.now() - t0) / 1000,
      platform, arch: archBasic, model: uaModel, threads, device_memory_gb: devMem, memory_gb_assumed: memGB, memory_known: memKnown,
      cpu_bandwidth_gbps: cpuBw, bandwidth_by_threads: bwBy, bandwidth_source: bandwidthSource,
      gpu_bandwidth_gbps: gpuBw, gpu_mem_gb_estimate: gpuMemGB, gpu_method: g.method || null,
      gpu_vendor: gpuVendor, gpu_family: gpuFamily,
      gflops_f32: gflops, gpu: g, bandwidth_gbps: ceiling, k: 0.8,
      extra,
    };
  }

  // ================================================================== in-browser model run
  // Loads wllama (a WebAssembly build of llama.cpp) from a CDN on demand and runs the same pinned
  // GGUF the downloadable speed check uses, entirely in this tab. Nothing is sent anywhere except the
  // model download itself (from Hugging Face) and, if the run finishes, a runs entry with the numbers.
  const MODEL_URL = 'https://huggingface.co/unsloth/Qwen3-0.6B-GGUF/resolve/50968a4468ef4233ed78cd7c3de230dd1d61a56b/Qwen3-0.6B-Q4_K_M.gguf';
  const MODEL_BYTES = 400 * 1024 * 1024; // ~400 MB, stated to the user before any download starts
  const WLLAMA_BASE = 'https://cdn.jsdelivr.net/npm/@wllama/wllama@2/esm/';
  let _wllama = null;
  async function runModelInBrowser(onProgress){
    const { Wllama } = await import(/* webpackIgnore: true */ WLLAMA_BASE + 'index.js');
    if (!_wllama) {
      _wllama = new Wllama({
        'single-thread/wllama.wasm': WLLAMA_BASE + 'single-thread/wllama.wasm',
        'multi-thread/wllama.wasm': WLLAMA_BASE + 'multi-thread/wllama.wasm',
      });
      await _wllama.loadModelFromUrl(MODEL_URL, {
        n_threads: Math.max(1, Math.min(8, navigator.hardwareConcurrency || 4)),
        n_ctx: 2048,
        progressCallback: ({ loaded, total }) => onProgress && onProgress('downloading', total ? loaded / total : null),
      });
    }
    onProgress && onProgress('running', null);
    const prompt = 'Write a short paragraph about the ocean.';
    let firstTokenAt = null, tokens = 0;
    const t0 = performance.now();
    const budgetMs = 20000;
    await _wllama.createCompletion(prompt, {
      nPredict: 512,
      sampling: { temp: 0.7 },
      onNewToken: (token, piece, currentText, { abortSignal }) => {
        if (firstTokenAt === null) firstTokenAt = performance.now();
        tokens++;
        if (performance.now() - t0 > budgetMs) abortSignal();
      },
    });
    const secs = (performance.now() - t0) / 1000;
    const ttft = firstTokenAt !== null ? (firstTokenAt - t0) / 1000 : null;
    return { model: 'Qwen3-0.6B-Q4_K_M', file_gb: 0.4, tps: secs > 0 ? tokens / secs : 0, ttft_s: ttft, threads: Math.max(1, Math.min(8, navigator.hardwareConcurrency || 4)), seconds: secs };
  }

  window.SJ = { measure, grade, RESERVE, getInstallId, MODEL_URL, MODEL_BYTES, runModelInBrowser,
    async post(receipt){ try { const r = await fetch('/v1/receipt', { method:'POST', headers:{'content-type':'application/json'}, body: JSON.stringify(receipt) }); return r.ok ? await r.json() : { ok:false, status:r.status }; } catch(e){ return { ok:false, error:String(e) }; } }
  };
})();
