import * as fs from 'fs';
import * as path from 'path';

/**
 * Simple file-based pool with account locking
 * Uses exclusive file creation for atomic locking
 */
export class SequencePool {
  private accounts: string[];
  private lockDir: string;

  constructor(relayerIds: string[]) {
    if (!relayerIds || relayerIds.length === 0) {
      throw new Error('No sequence accounts configured');
    }
    this.accounts = relayerIds;
    this.lockDir = path.join(__dirname, '..', '.pool-locks');

    // Ensure lock directory exists
    fs.mkdirSync(this.lockDir, { recursive: true });
  }

  async acquire(): Promise<{ relayerId: string }> {
    // Try each account
    for (const relayerId of this.accounts) {
      const lockFile = path.join(this.lockDir, `${relayerId}.lock`);

      try {
        // 'wx' flag = write exclusive (fails if file exists)
        fs.writeFileSync(lockFile, process.pid.toString(), { flag: 'wx' });
        return { relayerId };
      } catch (e) {
        // Already locked, try next
      }
    }

    throw new Error('Too many transactions queued. Please try again later');
  }

  release(account: { relayerId: string }): void {
    try {
      fs.unlinkSync(path.join(this.lockDir, `${account.relayerId}.lock`));
    } catch (e) {
      // Lock was already released or didn't exist
    }
  }

  getStats() {
    let inUse = 0;
    for (const relayerId of this.accounts) {
      if (fs.existsSync(path.join(this.lockDir, `${relayerId}.lock`))) {
        inUse++;
      }
    }

    return {
      total: this.accounts.length,
      available: this.accounts.length - inUse,
      inUse
    };
  }
}
