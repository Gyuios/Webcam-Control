// End-to-end smoke test for the Media Foundation virtual camera.
// It starts one camera-host for the physical source and a second one that
// consumes CameraTuner Virtual Camera, then samples the resulting CTFRAME2.

const { spawn } = require('child_process');
const readline = require('readline');
const fs = require('fs');
const path = require('path');

const projectRoot = path.resolve(__dirname, '..');
const hostPath = path.join(
  projectRoot,
  'src-tauri',
  'binaries',
  'camera-tuner-camera-host-x86_64-pc-windows-msvc.exe',
);
const cameraName = process.env.CAMERA_TUNER_TEST_CAMERA || 'UVC Camera';
const durationSeconds = Math.max(2, Number(process.env.CAMERA_TUNER_TEST_SECONDS || 6));
const sourceWidth = Math.max(2, Number(process.env.CAMERA_TUNER_TEST_SOURCE_WIDTH || 640));
const sourceHeight = Math.max(2, Number(process.env.CAMERA_TUNER_TEST_SOURCE_HEIGHT || 360));
const assertVisible = process.argv.includes('--assert-visible');

class HostClient {
  constructor(label) {
    this.label = label;
    this.requestId = 0;
    this.pending = [];
    this.process = spawn(hostPath, [], { stdio: ['pipe', 'pipe', 'pipe'] });
    this.lines = readline.createInterface({ input: this.process.stdout });
    this.lines.on('line', (line) => this.pending.shift()?.(JSON.parse(line)));
    this.process.stderr.on('data', (data) => process.stderr.write(`[${label}] ${data}`));
  }

  request(payload, timeoutMilliseconds = 15_000) {
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(
        () => reject(new Error(`${this.label}: timeout ejecutando ${payload.command}`)),
        timeoutMilliseconds,
      );
      this.pending.push((response) => {
        clearTimeout(timeout);
        resolve(response);
      });
      this.process.stdin.write(
        `${JSON.stringify({
          protocolVersion: 4,
          requestId: ++this.requestId,
          deadlineUnixMs: null,
          payload,
        })}\n`,
      );
    });
  }

  async stop() {
    try {
      await this.request({ command: 'close' }, 5_000);
    } catch {}
    try {
      await this.request({ command: 'shutdown' }, 5_000);
    } catch {}
    this.process.stdin.end();
  }
}

function unwrap(response) {
  if (!response.result || response.result.Err) {
    throw new Error(JSON.stringify(response));
  }
  return response.result.Ok;
}

function openCommand(
  deviceId,
  format,
  framePath,
  outputPixelFormat = 'BGRA',
  outputWidth = 640,
  outputHeight = 360,
) {
  return {
    command: 'open',
    arguments: {
      deviceId,
      backend: 'media-capture',
      format,
      outputWidth,
      outputHeight,
      outputPixelFormat,
      scaling: 'fast-bilinear',
      framePath,
      filterGraph: { nodes: [] },
      lutAssets: {},
      plugins: [],
    },
  };
}

function readLatestFrame(framePath) {
  const data = fs.readFileSync(framePath);
  if (data.subarray(0, 8).toString('ascii') !== 'CTFRAME2') {
    throw new Error('El consumidor no publicó CTFRAME2.');
  }
  const slotSpan = data.readUInt32LE(16);
  const publication = data.readBigUInt64LE(32);
  const slotValue = Number(publication & 3n);
  const sequence = Number(publication >> 2n);
  if (slotValue === 0 || sequence === 0) return null;

  const slotOffset = 64 + (slotValue - 1) * slotSpan;
  const width = data.readUInt32LE(slotOffset + 16);
  const height = data.readUInt32LE(slotOffset + 20);
  const frameSize = data.readUInt32LE(slotOffset + 32);
  const pixelFormat = data.readUInt32LE(slotOffset + 28);
  if (pixelFormat !== 1) {
    throw new Error(`El consumidor de prueba publicó un formato inesperado: ${pixelFormat}.`);
  }
  const pixels = data.subarray(slotOffset + 64, slotOffset + 64 + frameSize);
  let blue = 0;
  let green = 0;
  let red = 0;
  let samples = 0;
  for (let offset = 0; offset < pixels.length; offset += 4 * 64) {
    blue += pixels[offset];
    green += pixels[offset + 1];
    red += pixels[offset + 2];
    samples += 1;
  }
  return {
    sequence,
    width,
    height,
    blue: blue / samples,
    green: green / samples,
    red: red / samples,
  };
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

async function main() {
  const producer = new HostClient('producer');
  const consumer = new HostClient('consumer');
  const producerPath = 'C:\\ProgramData\\CameraTuner\\frame-v3.bin';
  const consumerPath = 'C:\\ProgramData\\CameraTuner\\virtual-test-v2.bin';
  try {
    const devices = unwrap(await producer.request({ command: 'enumerateDevices' })).data;
    const physical = devices.find((device) => device.name === cameraName);
    const virtual = devices.find((device) =>
      device.name.startsWith('CameraTuner Virtual Camera'),
    );
    if (!physical || !virtual) throw new Error('No se encontraron ambas cámaras requeridas.');

    const physicalFormats = unwrap(
      await producer.request({
        command: 'enumerateFormats',
        arguments: { deviceId: physical.id },
      }),
    ).data;
    const physicalFormat = physicalFormats.find(
      (format) =>
        format.width === sourceWidth &&
        format.height === sourceHeight &&
        format.fpsNumerator === 30 &&
        format.pixelFormat === 'NV12',
    );
    if (!physicalFormat) {
      throw new Error(`La cámara física no ofrece ${sourceWidth}×${sourceHeight} NV12 a 30 FPS.`);
    }
    unwrap(
      await producer.request(
        openCommand(
          physical.id,
          physicalFormat,
          producerPath,
          'NV12',
          sourceWidth,
          sourceHeight,
        ),
      ),
    );

    const virtualFormats = unwrap(
      await consumer.request(
        { command: 'enumerateFormats', arguments: { deviceId: virtual.id } },
        30_000,
      ),
    ).data;
    const virtualFormat =
      virtualFormats.find(
        (format) =>
          format.width === 640 && format.height === 360 && format.fpsNumerator === 30,
      ) || virtualFormats[0];
    if (!virtualFormat) throw new Error('La cámara virtual no expuso formatos.');
    unwrap(
      await consumer.request(openCommand(virtual.id, virtualFormat, consumerPath), 30_000),
    );

    let sampleCount = 0;
    let uniqueSequences = 0;
    let blueSamples = 0;
    let darkSamples = 0;
    let lastSequence = 0;
    let latest = null;
    for (let index = 0; index < durationSeconds * 10; index += 1) {
      await delay(100);
      latest = readLatestFrame(consumerPath);
      if (!latest) continue;
      sampleCount += 1;
      if (latest.sequence !== lastSequence) {
        uniqueSequences += 1;
        lastSequence = latest.sequence;
      }
      if (
        latest.blue > 80 &&
        latest.blue > latest.green * 1.8 &&
        latest.blue > latest.red * 1.8
      ) {
        blueSamples += 1;
      }
      if (latest.blue + latest.green + latest.red < 9) darkSamples += 1;
    }

    const result = {
      physicalFormat,
      virtualFormat,
      durationSeconds,
      sampleCount,
      uniqueSequences,
      blueSamples,
      darkSamples,
      latest,
    };
    console.log(JSON.stringify(result, null, 2));
    if (sampleCount < durationSeconds * 8 || uniqueSequences < durationSeconds * 5) {
      throw new Error('La cámara virtual no mantuvo una cadencia mínima estable.');
    }
    if (assertVisible && (blueSamples > 0 || darkSamples > 0)) {
      throw new Error('Se detectaron cuadros azules o negros durante la prueba visible.');
    }
  } finally {
    await consumer.stop();
    await producer.stop();
  }
}

main().catch((error) => {
  console.error(error.stack || error);
  process.exitCode = 1;
});
