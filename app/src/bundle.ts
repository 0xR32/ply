import { dirname, join } from 'node:path';

// The .app's entry (`just dmg`): GPUIX's addon ships in Contents/Frameworks, which napi-rs's loader cannot find on its own.
process.env.NAPI_RS_NATIVE_LIBRARY_PATH ??= join(
  dirname(process.execPath),
  '..',
  'Frameworks',
  'gpuix-native.darwin-arm64.node',
);

// A static import would be hoisted above the line that sets the addon path; --bytecode output has no top-level await.
import('./main').catch((error: unknown) => {
  console.error('ply failed to start:', error);
  process.exit(1);
});
