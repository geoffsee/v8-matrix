import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { rspack } from '@rspack/core';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const isProd = process.env.NODE_ENV === 'production';

export default {
  mode: isProd ? 'production' : 'development',
  entry: path.join(__dirname, 'src/main.jsx'),
  output: {
    path: path.resolve(__dirname, '../crates/v8-matrix-wasm-server/static'),
    filename: isProd ? 'assets/[name].[contenthash:8].js' : 'assets/[name].js',
    chunkFilename: isProd ? 'assets/[name].[contenthash:8].js' : 'assets/[name].js',
    clean: true,
  },
  resolve: {
    extensions: ['.js', '.jsx'],
  },
  module: {
    rules: [
      {
        test: /\.jsx?$/,
        exclude: /node_modules/,
        type: 'javascript/auto',
        use: [
          {
            loader: 'builtin:swc-loader',
            options: {
              jsc: {
                parser: {
                  syntax: 'ecmascript',
                  jsx: true,
                },
                transform: {
                  react: {
                    runtime: 'automatic',
                    development: !isProd,
                  },
                },
              },
            },
          },
        ],
      },
      {
        test: /\.css$/,
        type: 'css',
      },
    ],
  },
  plugins: [
    new rspack.HtmlRspackPlugin({
      templateContent: ({ htmlRspackPlugin }) => `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>v8-matrix</title>
    ${htmlRspackPlugin.tags.headTags}
  </head>
  <body>
    <div id="app"></div>
    ${htmlRspackPlugin.tags.bodyTags}
  </body>
</html>`,
    }),
  ],
  experiments: {
    css: true,
  },
  devtool: isProd ? false : 'source-map',
};
