// 00_primordials.js — Capture and freeze primordial references before user code runs.
// This prevents user code from monkey-patching built-ins that the runtime depends on.

((globalThis) => {
  "use strict";

  const {
    Object,
    Array,
    JSON,
    String,
    Number,
    Boolean,
    Map,
    Set,
    Promise,
    Symbol,
    TypeError,
    RangeError,
    Error,
    Math,
    Date,
    RegExp,
    ArrayBuffer,
    Uint8Array,
    DataView,
  } = globalThis;

  const primordials = {
    ObjectDefineProperty: Object.defineProperty,
    ObjectDefineProperties: Object.defineProperties,
    ObjectFreeze: Object.freeze,
    ObjectKeys: Object.keys,
    ObjectEntries: Object.entries,
    ObjectFromEntries: Object.fromEntries,
    ObjectCreate: Object.create,
    ObjectAssign: Object.assign,
    ObjectGetPrototypeOf: Object.getPrototypeOf,
    ObjectHasOwn: Object.hasOwn,
    ArrayPrototypeMap: Array.prototype.map,
    ArrayPrototypePush: Array.prototype.push,
    ArrayPrototypeJoin: Array.prototype.join,
    ArrayPrototypeSlice: Array.prototype.slice,
    ArrayPrototypeForEach: Array.prototype.forEach,
    ArrayPrototypeFilter: Array.prototype.filter,
    ArrayPrototypeFind: Array.prototype.find,
    ArrayPrototypeIncludes: Array.prototype.includes,
    ArrayIsArray: Array.isArray,
    JSONStringify: JSON.stringify,
    JSONParse: JSON.parse,
    StringPrototypeToLowerCase: String.prototype.toLowerCase,
    StringPrototypeToUpperCase: String.prototype.toUpperCase,
    StringPrototypeTrim: String.prototype.trim,
    StringPrototypeStartsWith: String.prototype.startsWith,
    StringPrototypeSplit: String.prototype.split,
    StringPrototypeSlice: String.prototype.slice,
    PromiseResolve: Promise.resolve.bind(Promise),
    PromiseReject: Promise.reject.bind(Promise),
    PromiseAll: Promise.all.bind(Promise),
    PromisePrototypeThen: Promise.prototype.then,
    MapPrototypeGet: Map.prototype.get,
    MapPrototypeSet: Map.prototype.set,
    MapPrototypeHas: Map.prototype.has,
    MapPrototypeDelete: Map.prototype.delete,
    MapPrototypeEntries: Map.prototype.entries,
    SetPrototypeAdd: Set.prototype.add,
    SetPrototypeHas: Set.prototype.has,
    Symbol,
    TypeError,
    RangeError,
    Error,
    Math,
    Date,
    RegExp,
    ArrayBuffer,
    Uint8Array,
    DataView,
  };

  Object.freeze(primordials);

  // Stash on a non-enumerable, non-configurable property
  Object.defineProperty(globalThis, "__primordials", {
    value: primordials,
    writable: false,
    enumerable: false,
    configurable: false,
  });
})(globalThis);
