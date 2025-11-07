/**
 * @typedef {Object} VersionConfig
 * @property {string} label - Display label for the version
 * @property {string} value - Internal value identifier
 * @property {string} path - URL path for the version
 * @property {boolean} isStable - Whether this is a stable release
 */

export const latestStable = "1.1.x";

/** @type {VersionConfig[]} */

export const allVersions = [
  { label: "v1.1.x (latest stable)", value: "1.1.x", path: "/relayer/1.1.x", isStable: true },
  { label: "v1.0.x", value: "1.0.x", path: "/relayer/1.0.x", isStable: true },
  { label: "Development", value: "development", path: "/relayer", isStable: false }
];
