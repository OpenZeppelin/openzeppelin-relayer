import net from "node:net";
import { v4 as uuidv4 } from "uuid";
import { NetworkTransactionRequest } from "@openzeppelin/relayer-sdk/dist/src/models/network-transaction-request";

type SendTransactionResult = {
  id: string;
  relayer_id: string;
  status: string;
}

type Result<T> = {
  request_id: string;
  result: T;
  error: string | null;
}

type Relayer = {
  sendTransaction: (payload: NetworkTransactionRequest) => Promise<Result<SendTransactionResult>>;
}

export async function runPlugin(main: (plugin: PluginAPI) => Promise<void>) {
  try {
    // checks if socket path is provided
    let socketPath = process.argv[2];
    if (!socketPath) {
      throw new Error("Socket path is required");
    }

    // creates plugin instance
    let plugin = new PluginAPI(socketPath);

    // runs main function
    await main(plugin)
      .then(() => plugin.close())
      .catch((error) => {
        console.error(error);
        // closes socket signaling error
        plugin.closeErrored(error);
        })
      .finally(() => {
        plugin.close();
        process.exit(0);
      });
  } catch (error) {
      console.error(error);
      process.exit(1);
  }
}

export class PluginAPI {
  socket: net.Socket;
  pending: Map<string, { resolve: (value: any) => void, reject: (reason: any) => void }>;
  private _connectionPromise: Promise<void> | null = null;
  private _connected: boolean = false;

  constructor(socketPath: string) {
    this.socket = net.createConnection(socketPath);
    this.pending = new Map();

    this._connectionPromise = new Promise((resolve, reject) => {
      this.socket.on('connect', () => {
        this._connected = true;
        resolve();
      });

      this.socket.on('error', (error) => {
        console.error("Socket ERROR:", error);
        reject(error);
      });
    });

    this.socket.on('data', data => {
      data.toString().split('\n').filter(Boolean).forEach((msg: string) => {
        const parsed = JSON.parse(msg);
        const { requestId, result, error } = parsed;
        const resolver = this.pending.get(requestId);
        if (resolver) {
          error ? resolver.reject(error) : resolver.resolve(result);
          this.pending.delete(requestId);
        }
      });
    });
  }

  useRelayer(relayerId: string): Relayer {
    return {
      sendTransaction: (payload: NetworkTransactionRequest) => this._send<SendTransactionResult>(relayerId, "sendTransaction", payload),
    };
  }

  async _send<T>(relayerId: string, method: string, payload: any): Promise<Result<T>> {
    const requestId = uuidv4();
    const message = JSON.stringify({ requestId, relayerId, method, payload }) + "\n";

    if (!this._connected) {
      await this._connectionPromise;
    }

    const result = this.socket.write(message, (error) => {
      if (error) {
        console.error("Error sending message:", error);
      }
    });

    if (!result) {
      throw new Error(`Failed to send message to relayer: ${message}`);
    }

    return new Promise((resolve, reject) => {
      this.pending.set(requestId, { resolve, reject });
    });
  }

  close() {
    this.socket.end();
  }

  closeErrored(error: any) {
    this.socket.destroy(error);
  }
}
