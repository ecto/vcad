/** @type {import('next').NextConfig} */
const nextConfig = {
  // Transpile workspace packages
  transpilePackages: ["@vcad/ir", "@vcad/engine", "@vcad/kernel-wasm"],

  // WASM support
  webpack: (config, { isServer, webpack }) => {
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

    // On client: stub out node: protocol imports (used in Node-only branches)
    if (!isServer) {
      config.plugins.push(
        new webpack.NormalModuleReplacementPlugin(/^node:/, (resource) => {
          resource.request = resource.request.replace(/^node:/, "");
        })
      );

      config.resolve.fallback = {
        ...config.resolve.fallback,
        fs: false,
        "fs/promises": false,
        path: false,
        url: false,
        os: false,
        crypto: false,
        stream: false,
        buffer: false,
        util: false,
      };
    }

    return config;
  },

  // Images configuration for Vercel
  images: {
    unoptimized: false,
  },
};

export default nextConfig;
