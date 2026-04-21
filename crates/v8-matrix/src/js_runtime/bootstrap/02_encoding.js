// 02_encoding.js — TextEncoder / TextDecoder
// V8 doesn't provide these natively; we polyfill them here.

((globalThis) => {
  "use strict";

  const { ObjectDefineProperty } = globalThis.__primordials;

  // Minimal UTF-8 encoder (covers the full spec via the native V8 string encoding)
  class TextEncoder {
    get encoding() {
      return "utf-8";
    }

    encode(input = "") {
      const str = String(input);
      // Use the native binding if available, otherwise fallback to JS impl
      if (typeof __native_encode_utf8 === "function") {
        return __native_encode_utf8(str);
      }
      // Pure JS fallback — correct but slower
      const buf = [];
      for (let i = 0; i < str.length; i++) {
        let c = str.charCodeAt(i);
        if (c < 0x80) {
          buf.push(c);
        } else if (c < 0x800) {
          buf.push(0xc0 | (c >> 6), 0x80 | (c & 0x3f));
        } else if (c >= 0xd800 && c <= 0xdbff) {
          const next = str.charCodeAt(++i);
          const cp = ((c - 0xd800) << 10) + (next - 0xdc00) + 0x10000;
          buf.push(
            0xf0 | (cp >> 18),
            0x80 | ((cp >> 12) & 0x3f),
            0x80 | ((cp >> 6) & 0x3f),
            0x80 | (cp & 0x3f),
          );
        } else {
          buf.push(0xe0 | (c >> 12), 0x80 | ((c >> 6) & 0x3f), 0x80 | (c & 0x3f));
        }
      }
      return new Uint8Array(buf);
    }

    encodeInto(source, destination) {
      const encoded = this.encode(source);
      const len = Math.min(encoded.length, destination.length);
      destination.set(encoded.subarray(0, len));
      // Count how many source characters were consumed
      let read = 0;
      let written = 0;
      const str = String(source);
      const enc = this.encode(str);
      for (let i = 0; i < str.length && written < len; i++) {
        const charEnc = this.encode(str[i]);
        if (written + charEnc.length > len) break;
        written += charEnc.length;
        read++;
      }
      return { read, written };
    }
  }

  class TextDecoder {
    #encoding;
    #fatal;
    #ignoreBOM;

    constructor(encoding = "utf-8", options = {}) {
      const label = String(encoding).toLowerCase().trim();
      if (label !== "utf-8" && label !== "utf8") {
        throw new RangeError(`TextDecoder: unsupported encoding '${encoding}'`);
      }
      this.#encoding = "utf-8";
      this.#fatal = !!options.fatal;
      this.#ignoreBOM = !!options.ignoreBOM;
    }

    get encoding() {
      return this.#encoding;
    }
    get fatal() {
      return this.#fatal;
    }
    get ignoreBOM() {
      return this.#ignoreBOM;
    }

    decode(input) {
      if (input === undefined || input === null) return "";

      // Use the native binding if available
      if (typeof __native_decode_utf8 === "function") {
        let bytes;
        if (input instanceof Uint8Array) {
          bytes = input;
        } else if (input instanceof ArrayBuffer) {
          bytes = new Uint8Array(input);
        } else if (ArrayBuffer.isView(input)) {
          bytes = new Uint8Array(input.buffer, input.byteOffset, input.byteLength);
        } else {
          throw new TypeError("Expected ArrayBuffer or ArrayBufferView");
        }
        return __native_decode_utf8(bytes);
      }

      // Pure JS fallback
      let bytes;
      if (input instanceof Uint8Array) {
        bytes = input;
      } else if (input instanceof ArrayBuffer) {
        bytes = new Uint8Array(input);
      } else if (ArrayBuffer.isView(input)) {
        bytes = new Uint8Array(input.buffer, input.byteOffset, input.byteLength);
      } else {
        throw new TypeError("Expected ArrayBuffer or ArrayBufferView");
      }

      let result = "";
      let i = 0;

      // Skip BOM if present
      if (
        !this.#ignoreBOM &&
        bytes.length >= 3 &&
        bytes[0] === 0xef &&
        bytes[1] === 0xbb &&
        bytes[2] === 0xbf
      ) {
        i = 3;
      }

      while (i < bytes.length) {
        const b = bytes[i];
        if (b < 0x80) {
          result += String.fromCharCode(b);
          i++;
        } else if ((b & 0xe0) === 0xc0) {
          const c = ((b & 0x1f) << 6) | (bytes[i + 1] & 0x3f);
          result += String.fromCharCode(c);
          i += 2;
        } else if ((b & 0xf0) === 0xe0) {
          const c =
            ((b & 0x0f) << 12) | ((bytes[i + 1] & 0x3f) << 6) | (bytes[i + 2] & 0x3f);
          result += String.fromCharCode(c);
          i += 3;
        } else if ((b & 0xf8) === 0xf0) {
          let cp =
            ((b & 0x07) << 18) |
            ((bytes[i + 1] & 0x3f) << 12) |
            ((bytes[i + 2] & 0x3f) << 6) |
            (bytes[i + 3] & 0x3f);
          cp -= 0x10000;
          result += String.fromCharCode(0xd800 + (cp >> 10), 0xdc00 + (cp & 0x3ff));
          i += 4;
        } else {
          if (this.#fatal) throw new TypeError("Invalid UTF-8 byte sequence");
          result += "\uFFFD";
          i++;
        }
      }

      return result;
    }
  }

  ObjectDefineProperty(globalThis, "TextEncoder", {
    value: TextEncoder,
    writable: false,
    enumerable: false,
    configurable: false,
  });

  ObjectDefineProperty(globalThis, "TextDecoder", {
    value: TextDecoder,
    writable: false,
    enumerable: false,
    configurable: false,
  });
})(globalThis);
