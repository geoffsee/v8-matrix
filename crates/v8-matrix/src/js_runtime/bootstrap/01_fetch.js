// 01_fetch.js — Web-compatible Request, Response, Headers, and fetch()
// The actual network I/O is performed by native Rust bindings.
// This layer provides the JS API surface that user code interacts with.

((globalThis) => {
  "use strict";

  const { ObjectDefineProperty, ObjectFreeze, ObjectDefineProperties } =
    globalThis.__primordials;

  // ─── Headers ────────────────────────────────────────────────────────────────

  class Headers {
    #map = new Map();

    constructor(init) {
      if (init instanceof Headers) {
        for (const [k, v] of init) {
          this.set(k, v);
        }
      } else if (Array.isArray(init)) {
        for (const [k, v] of init) {
          this.append(String(k).toLowerCase(), String(v));
        }
      } else if (init && typeof init === "object") {
        for (const key of Object.keys(init)) {
          this.set(key, init[key]);
        }
      }
    }

    get(name) {
      const vals = this.#map.get(String(name).toLowerCase());
      return vals ? vals.join(", ") : null;
    }

    set(name, value) {
      this.#map.set(String(name).toLowerCase(), [String(value)]);
    }

    append(name, value) {
      const key = String(name).toLowerCase();
      const existing = this.#map.get(key);
      if (existing) {
        existing.push(String(value));
      } else {
        this.#map.set(key, [String(value)]);
      }
    }

    has(name) {
      return this.#map.has(String(name).toLowerCase());
    }

    delete(name) {
      this.#map.delete(String(name).toLowerCase());
    }

    forEach(cb, thisArg) {
      for (const [key, vals] of this.#map) {
        cb.call(thisArg, vals.join(", "), key, this);
      }
    }

    *entries() {
      for (const [key, vals] of this.#map) {
        yield [key, vals.join(", ")];
      }
    }

    *keys() {
      for (const key of this.#map.keys()) {
        yield key;
      }
    }

    *values() {
      for (const vals of this.#map.values()) {
        yield vals.join(", ");
      }
    }

    [Symbol.iterator]() {
      return this.entries();
    }

    toJSON() {
      const obj = {};
      for (const [k, v] of this.entries()) {
        obj[k] = v;
      }
      return obj;
    }
  }

  // ─── Request ────────────────────────────────────────────────────────────────

  class Request {
    #url;
    #method;
    #headers;
    #body;
    #bodyUsed = false;
    #signal;

    constructor(input, init = {}) {
      if (input instanceof Request) {
        this.#url = input.url;
        this.#method = input.method;
        this.#headers = new Headers(input.headers);
        this.#body = input.#body;
        this.#signal = input.#signal;
      } else {
        this.#url = String(input);
        this.#method = "GET";
        this.#headers = new Headers();
        this.#body = null;
        this.#signal = null;
      }

      if (init.method !== undefined) this.#method = String(init.method).toUpperCase();
      if (init.headers !== undefined) this.#headers = new Headers(init.headers);
      if (init.body !== undefined) this.#body = init.body;
      if (init.signal !== undefined) this.#signal = init.signal;
    }

    get url() { return this.#url; }
    get method() { return this.#method; }
    get headers() { return this.#headers; }
    get body() { return this.#body; }
    get bodyUsed() { return this.#bodyUsed; }
    get signal() { return this.#signal; }

    async text() {
      this.#bodyUsed = true;
      if (this.#body === null || this.#body === undefined) return "";
      if (typeof this.#body === "string") return this.#body;
      if (this.#body instanceof ArrayBuffer) {
        return new TextDecoder().decode(this.#body);
      }
      if (this.#body instanceof Uint8Array) {
        return new TextDecoder().decode(this.#body);
      }
      return String(this.#body);
    }

    async json() {
      const t = await this.text();
      return JSON.parse(t);
    }

    async arrayBuffer() {
      this.#bodyUsed = true;
      if (this.#body instanceof ArrayBuffer) return this.#body;
      if (this.#body instanceof Uint8Array) return this.#body.buffer;
      const text = typeof this.#body === "string" ? this.#body : String(this.#body ?? "");
      return new TextEncoder().encode(text).buffer;
    }

    clone() {
      if (this.#bodyUsed) throw new TypeError("Body already consumed");
      return new Request(this);
    }
  }

  // ─── Response ───────────────────────────────────────────────────────────────

  class Response {
    #status;
    #statusText;
    #headers;
    #body;
    #bodyUsed = false;
    #ok;
    #url;
    #redirected;

    constructor(body, init = {}) {
      this.#status = init.status !== undefined ? Number(init.status) : 200;
      this.#statusText = init.statusText !== undefined ? String(init.statusText) : "";
      this.#headers = new Headers(init.headers);
      this.#ok = this.#status >= 200 && this.#status < 300;
      this.#url = init.url || "";
      this.#redirected = !!init.redirected;

      if (body === null || body === undefined) {
        this.#body = null;
      } else if (typeof body === "string") {
        this.#body = body;
        if (!this.#headers.has("content-type")) {
          this.#headers.set("content-type", "text/plain;charset=UTF-8");
        }
      } else if (body instanceof ArrayBuffer || body instanceof Uint8Array) {
        this.#body = body;
      } else {
        // Assume object → JSON
        this.#body = JSON.stringify(body);
        if (!this.#headers.has("content-type")) {
          this.#headers.set("content-type", "application/json");
        }
      }
    }

    get status() { return this.#status; }
    get statusText() { return this.#statusText; }
    get headers() { return this.#headers; }
    get ok() { return this.#ok; }
    get bodyUsed() { return this.#bodyUsed; }
    get url() { return this.#url; }
    get redirected() { return this.#redirected; }
    get body() { return this.#body; }

    async text() {
      this.#bodyUsed = true;
      if (this.#body === null) return "";
      if (typeof this.#body === "string") return this.#body;
      if (this.#body instanceof ArrayBuffer) {
        return new TextDecoder().decode(this.#body);
      }
      if (this.#body instanceof Uint8Array) {
        return new TextDecoder().decode(this.#body);
      }
      return String(this.#body);
    }

    async json() {
      const t = await this.text();
      return JSON.parse(t);
    }

    async arrayBuffer() {
      this.#bodyUsed = true;
      if (this.#body instanceof ArrayBuffer) return this.#body;
      if (this.#body instanceof Uint8Array) return this.#body.buffer;
      const text = typeof this.#body === "string" ? this.#body : "";
      return new TextEncoder().encode(text).buffer;
    }

    clone() {
      if (this.#bodyUsed) throw new TypeError("Body already consumed");
      return new Response(this.#body, {
        status: this.#status,
        statusText: this.#statusText,
        headers: this.#headers,
        url: this.#url,
        redirected: this.#redirected,
      });
    }

    static json(data, init = {}) {
      const body = JSON.stringify(data);
      const headers = new Headers(init.headers);
      if (!headers.has("content-type")) {
        headers.set("content-type", "application/json");
      }
      return new Response(body, { ...init, headers });
    }

    static redirect(url, status = 302) {
      const headers = new Headers({ location: String(url) });
      return new Response(null, { status, headers });
    }
  }

  // ─── fetch() — backed by native __native_fetch ─────────────────────────────

  // __native_fetch is injected by the Rust bindings layer. It accepts
  // (url, method, headersJson, body) and returns a promise that resolves to
  // { status, statusText, headers: [[k,v],...], body }
  async function fetch(input, init = {}) {
    let url, method, headers, body;

    if (input instanceof Request) {
      url = input.url;
      method = init.method || input.method;
      headers = new Headers(input.headers);
      body = input.body;
    } else {
      url = String(input);
      method = "GET";
      headers = new Headers();
      body = null;
    }

    if (init.method !== undefined) method = String(init.method).toUpperCase();
    if (init.headers !== undefined) headers = new Headers(init.headers);
    if (init.body !== undefined) body = init.body;

    // Serialize body to string for the native bridge
    let bodyStr = null;
    if (body !== null && body !== undefined) {
      if (typeof body === "string") {
        bodyStr = body;
      } else if (body instanceof ArrayBuffer || body instanceof Uint8Array) {
        bodyStr = new TextDecoder().decode(body);
      } else {
        bodyStr = JSON.stringify(body);
      }
    }

    const headersJson = JSON.stringify(headers.toJSON());

    // Call into Rust
    const raw = await __native_fetch(url, method, headersJson, bodyStr);

    // raw is { status, statusText, headers: [[k,v],...], body }
    const respHeaders = new Headers(raw.headers);
    return new Response(raw.body, {
      status: raw.status,
      statusText: raw.statusText,
      headers: respHeaders,
      url: url,
    });
  }

  // ─── Install on globalThis ──────────────────────────────────────────────────

  ObjectDefineProperty(globalThis, "Headers", {
    value: Headers,
    writable: false,
    enumerable: false,
    configurable: false,
  });

  ObjectDefineProperty(globalThis, "Request", {
    value: Request,
    writable: false,
    enumerable: false,
    configurable: false,
  });

  ObjectDefineProperty(globalThis, "Response", {
    value: Response,
    writable: false,
    enumerable: false,
    configurable: false,
  });

  ObjectDefineProperty(globalThis, "fetch", {
    value: fetch,
    writable: false,
    enumerable: false,
    configurable: false,
  });
})(globalThis);
