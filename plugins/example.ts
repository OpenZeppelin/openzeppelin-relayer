import { ethers } from "ethers";

function sleep(ms: number) {
    return new Promise((resolve) => setTimeout(resolve, ms));
}

async function main() {
    console.log("Running plugin with ethers:", ethers.version);
    await sleep(5000);
    console.log("Plugin finished");
}

main();
