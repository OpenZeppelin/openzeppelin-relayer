/**
 * Example plugin demonstrating the new 'handler' export pattern
 * 
 * This plugin shows how to use the simplified plugin pattern where you just
 * export a 'handler' function - no manual runPlugin() call needed!
 * 
 * Features demonstrated:
 * - Simple transaction sending
 * - Parameter validation  
 * - Logging and status updates
 * - Transaction confirmation waiting
 * - Error handling
 */

import { PluginAPI, Speed } from "@openzeppelin/relayer-sdk";

/**
 * Plugin parameters interface
 */
type HandlerParams = {
    destinationAddress: string;
    amount?: number;
    message?: string;
    relayerId?: string;
};

/**
 * Plugin result interface
 */
type HandlerResult = {
    success: boolean;
    transactionId: string;
    transactionHash: string | null;
    message: string;
    timestamp: string;
};

/**
 * 🎯 Plugin handler function - this is the entry point!
 * 
 * The relayer automatically calls this function when the plugin is invoked.
 * No manual runPlugin() call needed - just export this function as 'handler'.
 * 
 * @param api - Plugin API for interacting with relayers
 * @param params - Plugin parameters from the API call
 * @returns Promise with the plugin result
 */
export async function handler(api: PluginAPI, params: HandlerParams): Promise<HandlerResult> {
    console.info("🚀 Starting example handler plugin...");
    console.info(`📋 Parameters:`, JSON.stringify(params, null, 2));
    
    try {
        // Validate required parameters
        if (!params.destinationAddress) {
            throw new Error("destinationAddress is required");
        }
        
        // Default values
        const relayerId = params.relayerId || "sepolia-example";
        const amount = params.amount || 1;
        const message = params.message || "Hello from OpenZeppelin Relayer Plugin!";
        
        console.info(`💰 Sending ${amount} wei to ${params.destinationAddress}`);
        console.info(`📝 Message: ${message}`);
        console.info(`🔗 Using relayer: ${relayerId}`);
        
        // Get the relayer instance
        const relayer = api.useRelayer(relayerId);
        
        // Send the transaction
        console.info("📤 Submitting transaction...");
        const result = await relayer.sendTransaction({
            to: params.destinationAddress,
            value: amount,
            data: "0x", // Empty data for simple ETH transfer
            gas_limit: 21000,
            speed: Speed.FAST,
        });
        
        console.info(`✅ Transaction submitted!`);
        console.info(`📋 Transaction ID: ${result.id}`);
        console.info(`⏳ Status: ${result.status}`);
        
        // Wait for the transaction to be mined
        console.info("⏳ Waiting for transaction confirmation...");
        const confirmation = await result.wait({
            interval: 5000,  // Check every 5 seconds
            timeout: 120000  // Timeout after 2 minutes
        });
        
        console.info(`🎉 Transaction confirmed!`);
        console.info(`📋 Final status: ${confirmation.status}`);
        console.info(`🔗 Transaction hash: ${confirmation.hash || 'pending'}`);
        
        // Return success result
        return {
            success: true,
            transactionId: result.id,
            transactionHash: confirmation.hash || null,
            message: `Successfully sent ${amount} wei to ${params.destinationAddress}. ${message}`,
            timestamp: new Date().toISOString()
        };
        
    } catch (error) {
        console.error("❌ Plugin execution failed:", error);
        
        // Return error result
        return {
            success: false,
            transactionId: "",
            transactionHash: null,
            message: `Plugin failed: ${(error as Error).message}`,
            timestamp: new Date().toISOString()
        };
    }
}
