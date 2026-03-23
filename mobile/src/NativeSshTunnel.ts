import type { TurboModule } from 'react-native';
import { TurboModuleRegistry } from 'react-native';

export interface Spec extends TurboModule {
  /**
   * Establish an SSH tunnel.
   *
   * @param host        SSH server hostname or IP address
   * @param port        SSH server port (typically 22)
   * @param hostPubKey  Base64-encoded OpenSSH wire-format Ed25519 public key
   *                    (the second field of an ssh-ed25519 .pub file — "hk" in the QR)
   * @param clientPrivKey Base64-encoded OpenSSH private key PEM file contents
   *                      including BEGIN/END headers ("ck" in the QR)
   * @returns           The local port number that was bound for forwarding to
   *                    remote localhost:8000
   */
  connect(
    host: string,
    port: number,
    hostPubKey: string,
    clientPrivKey: string,
  ): Promise<number>;

  /**
   * Tear down the active SSH tunnel and free all resources.
   */
  disconnect(): void;
}

const NativeSshTunnel = TurboModuleRegistry.getEnforcing<Spec>('SshTunnel');

export default NativeSshTunnel;
