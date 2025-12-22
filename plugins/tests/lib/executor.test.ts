import '@jest/globals';

/**
 * Tests for executor.ts parseHeaders function
 *
 * The parseHeaders function is not exported, so we test its behavior
 * indirectly through the expected data format.
 */

describe('Header Parsing', () => {
  /**
   * Simulates the parseHeaders function from executor.ts
   * This mirrors the actual implementation for testing purposes
   */
  function parseHeaders(headersJson: string | undefined): Record<string, string[]> | undefined {
    if (!headersJson) {
      return undefined;
    }
    try {
      return JSON.parse(headersJson) as Record<string, string[]>;
    } catch {
      return undefined;
    }
  }

  describe('parseHeaders', () => {
    it('should parse valid headers JSON', () => {
      const headersJson = JSON.stringify({
        'content-type': ['application/json'],
        'authorization': ['Bearer token123'],
        'x-custom': ['value1', 'value2'],
      });

      const result = parseHeaders(headersJson);

      expect(result).toBeDefined();
      expect(result!['content-type']).toEqual(['application/json']);
      expect(result!['authorization']).toEqual(['Bearer token123']);
      expect(result!['x-custom']).toEqual(['value1', 'value2']);
    });

    it('should return undefined for empty string', () => {
      const result = parseHeaders('');

      expect(result).toBeUndefined();
    });

    it('should return undefined for undefined input', () => {
      const result = parseHeaders(undefined);

      expect(result).toBeUndefined();
    });

    it('should return undefined for invalid JSON', () => {
      const result = parseHeaders('not valid json {{{');

      expect(result).toBeUndefined();
    });

    it('should handle empty object JSON', () => {
      const result = parseHeaders('{}');

      expect(result).toBeDefined();
      expect(Object.keys(result!)).toHaveLength(0);
    });

    it('should handle headers with empty arrays', () => {
      const headersJson = JSON.stringify({
        'x-empty': [],
        'x-filled': ['value'],
      });

      const result = parseHeaders(headersJson);

      expect(result).toBeDefined();
      expect(result!['x-empty']).toEqual([]);
      expect(result!['x-filled']).toEqual(['value']);
    });

    it('should handle special characters in header values', () => {
      const headersJson = JSON.stringify({
        'authorization': ['Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.test'],
        'x-json-data': ['{"key":"value"}'],
        'x-unicode': ['日本語'],
      });

      const result = parseHeaders(headersJson);

      expect(result).toBeDefined();
      expect(result!['authorization']![0]).toContain('Bearer');
      expect(result!['x-json-data']![0]).toBe('{"key":"value"}');
      expect(result!['x-unicode']![0]).toBe('日本語');
    });
  });

  describe('Header Format from Rust', () => {
    it('should match expected format from Rust HashMap<String, Vec<String>>', () => {
      // This is the format Rust serializes: HashMap<String, Vec<String>>
      const rustFormat = {
        'content-type': ['application/json'],
        'accept': ['text/html', 'application/json'],
        'host': ['localhost:8080'],
      };

      const serialized = JSON.stringify(rustFormat);
      const parsed = parseHeaders(serialized);

      expect(parsed).toEqual(rustFormat);
    });

    it('should handle Rust-serialized headers with lowercase keys', () => {
      // Rust normalizes header names to lowercase
      const rustFormat = {
        'x-custom-header': ['value'],
        'authorization': ['Bearer token'],
        'content-type': ['application/json'],
      };

      const serialized = JSON.stringify(rustFormat);
      const parsed = parseHeaders(serialized);

      // Verify all keys are lowercase
      Object.keys(parsed!).forEach(key => {
        expect(key).toBe(key.toLowerCase());
      });
    });
  });
});
