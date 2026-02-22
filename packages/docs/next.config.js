/** @type {import('next').NextConfig} */
const nextConfig = {
  // Transpile workspace packages
  transpilePackages: ["@vcad/ir"],

  // WASM support
  webpack: (config, { isServer }) => {
    // Enable WASM
    config.experiments = {
      ...config.experiments,
      asyncWebAssembly: true,
      topLevelAwait: true,
    };

    // Handle WASM files
    config.module.rules.push({
      test: /\.wasm$/,
      type: "asset/resource",
    });

    // Externalize engine packages on server - they only work in browser
    if (isServer) {
      config.externals = config.externals || [];
      config.externals.push({
        "@vcad/engine": "commonjs @vcad/engine",
        "@vcad/kernel-wasm": "commonjs @vcad/kernel-wasm",
      });
    }

    // Resolve node: protocol imports (for compatibility)
    config.resolve = config.resolve || {};
    config.resolve.fallback = {
      ...config.resolve.fallback,
      fs: false,
      path: false,
      url: false,
    };

    return config;
  },

  // Images configuration for Vercel
  images: {
    unoptimized: false,
  },
};

export default nextConfig;
