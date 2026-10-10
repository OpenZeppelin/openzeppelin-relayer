/**
 * @typedef {Object} VersionConfig
 * @property {string} label - Display label for the version
 * @property {string} value - Internal value identifier
 * @property {string} path - URL path for the version
 * @property {boolean} isStable - Whether this is a stable release
 */

export const latestStable = "1.8.x";

/** @type {VersionConfig[]} */

export const allVersions = [
  { label: "v1.8.x (latest stable)", value: "1.8.x", path: "/relayer/1.8.x", isStable: true },
  { label: "v1.7.x", value: "1.7.x", path: "/relayer/1.7.x", isStable: true },
  { label: "v1.6.x", value: "1.6.x", path: "/relayer/1.6.x", isStable: true },
  { label: "v1.5.x", value: "1.5.x", path: "/relayer/1.5.x", isStable: true },
  { label: "v1.4.x", value: "1.4.x", path: "/relayer/1.4.x", isStable: true },
  { label: "v1.3.x", value: "1.3.x", path: "/relayer/1.3.x", isStable: true },
  { label: "v1.2.x", value: "1.2.x", path: "/relayer/1.2.x", isStable: true },
  { label: "v1.1.x", value: "1.1.x", path: "/relayer/1.1.x", isStable: true },
  { label: "v1.0.x", value: "1.0.x", path: "/relayer/1.0.x", isStable: true },
  { label: "Development", value: "development", path: "/relayer", isStable: false }
];
