import type { NextConfig } from 'next';

const backend = process.env.STARTER_BACKEND_URL || (process.env.NODE_ENV === 'development'
  ? 'http://127.0.0.1:8080'
  : 'https://backend.starter.teamofsilicons.com');

const config: NextConfig = {
  poweredByHeader: false,
  async rewrites() {
    return {
      beforeFiles: [
        { source: '/api/:path*', destination: `${backend}/api/:path*` },
        { source: '/auth/:path*', destination: `${backend}/auth/:path*` },
        { source: '/install.sh', destination: 'https://github.com/teamofsilicons/silicon-starter/releases/latest/download/install.sh' },
      ],
      afterFiles: [],
      fallback: [],
    };
  },
};

export default config;
