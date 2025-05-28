// TODO improve this file
use core::{fmt, time::Duration};

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    strum::IntoStaticStr,
    strum::VariantNames,
    strum::VariantArray,
    strum::EnumString,
    strum::EnumIter,
    strum::EnumCount,
    serde::Deserialize,
)]
#[strum(serialize_all = "kebab-case")]
#[serde(rename_all = "snake_case")]
#[derive(num_enum::TryFromPrimitive)] // TryFrom<u64>
#[repr(u64)]
pub enum EvmNamedNetwork {
    Mainnet = 1,
    Holesky = 17000,
    Sepolia = 11155111,

    Optimism = 10,
    #[serde(alias = "optimism-sepolia")]
    OptimismSepolia = 11155420,

    #[serde(alias = "arbitrum-one")]
    Arbitrum = 42161,
    #[serde(alias = "arbitrum-sepolia")]
    ArbitrumSepolia = 421614,
    #[serde(alias = "arbitrum-nova")]
    ArbitrumNova = 42170,

    #[strum(
        to_string = "bsc",
        serialize = "binance-smart-chain",
        serialize = "bnb-smart-chain"
    )]
    #[serde(
        alias = "bsc",
        alias = "bnb-smart-chain",
        alias = "binance-smart-chain"
    )]
    BinanceSmartChain = 56,
    #[strum(
        to_string = "bsc-testnet",
        serialize = "binance-smart-chain-testnet",
        serialize = "bnb-smart-chain-testnet"
    )]
    #[serde(
        alias = "bsc_testnet",
        alias = "bsc-testnet",
        alias = "bnb-smart-chain-testnet",
        alias = "binance-smart-chain-testnet"
    )]
    BinanceSmartChainTestnet = 97,

    Scroll = 534352,
    #[serde(alias = "scroll-sepolia")]
    ScrollSepolia = 534351,
    #[serde(alias = "gnosis-chain")]
    Gnosis = 100,

    Polygon = 137,
    #[strum(to_string = "amoy", serialize = "polygon-amoy")]
    #[serde(alias = "amoy", alias = "polygon-amoy")]
    PolygonAmoy = 80002,
    #[strum(serialize = "polygon-zkevm", serialize = "zkevm")]
    #[serde(alias = "zkevm", alias = "polygon_zkevm", alias = "polygon-zkevm")]
    PolygonZkEvm = 1101,
    #[strum(serialize = "polygon-zkevm-testnet", serialize = "zkevm-testnet")]
    #[serde(
        alias = "zkevm-testnet",
        alias = "polygon_zkevm_testnet",
        alias = "polygon-zkevm-testnet"
    )]
    PolygonZkEvmTestnet = 1442,

    Fantom = 250,
    FantomTestnet = 4002,

    Moonbeam = 1284,
    MoonbeamDev = 1281,

    Moonriver = 1285,

    Moonbase = 1287,

    Avalanche = 43114,
    #[strum(to_string = "fuji", serialize = "avalanche-fuji")]
    #[serde(alias = "fuji")]
    AvalancheFuji = 43113,

    Celo = 42220,
    CeloAlfajores = 44787,
    CeloBaklava = 62320,

    Aurora = 1313161554,
    AuroraTestnet = 1313161555,

    Base = 8453,
    #[serde(alias = "base-sepolia")]
    BaseSepolia = 84532,
    Linea = 59144,
    #[serde(alias = "linea-sepolia")]
    LineaSepolia = 59141,

    #[strum(to_string = "zksync")]
    #[serde(alias = "zksync")]
    ZkSync = 324,
    #[strum(to_string = "zksync-testnet")]
    #[serde(alias = "zksync_testnet", alias = "zksync-testnet")]
    ZkSyncTestnet = 300,

    #[strum(to_string = "mantle")]
    #[serde(alias = "mantle")]
    Mantle = 5000,
    #[strum(to_string = "mantle-sepolia")]
    #[serde(alias = "mantle-sepolia")]
    MantleSepolia = 5003,

    #[strum(to_string = "unichain")]
    #[serde(alias = "unichain")]
    Unichain = 130,
    #[strum(to_string = "unichain-sepolia")]
    #[serde(alias = "unichain-sepolia")]
    UnichainSepolia = 1301,

    #[strum(to_string = "worldchain")]
    #[serde(alias = "worldchain")]
    WorldChain = 480,
    #[strum(to_string = "worldchain-sepolia")]
    #[serde(alias = "worldchain-sepolia", alias = "worldchain_sepolia")]
    WorldChainSepolia = 4801,
}

impl Default for EvmNamedNetwork {
    fn default() -> Self {
        Self::Mainnet
    }
}

impl EvmNamedNetwork {
    /// Returns the string representation of the chain.
    pub fn as_str(&self) -> &'static str {
        self.into()
    }

    /// Returns `true` if this chain is Ethereum or an Ethereum testnet.
    pub const fn is_ethereum(&self) -> bool {
        use EvmNamedNetwork::*;

        matches!(self, Mainnet | Holesky | Sepolia)
    }

    /// Returns true if the chain contains Optimism configuration.
    pub const fn is_optimism(self) -> bool {
        use EvmNamedNetwork::*;

        matches!(
            self,
            Optimism
                | OptimismSepolia
                | Base
                | BaseSepolia
                | Unichain
                | UnichainSepolia
                | WorldChain
                | WorldChainSepolia
        )
    }

    /// Returns true if the chain contains Arbitrum configuration.
    pub const fn is_arbitrum(self) -> bool {
        use EvmNamedNetwork::*;

        matches!(self, Arbitrum | ArbitrumSepolia | ArbitrumNova)
    }

    pub const fn average_blocktime(self) -> Option<Duration> {
        use EvmNamedNetwork::*;

        Some(Duration::from_millis(match self {
            Mainnet => 12_000,

            Arbitrum | ArbitrumSepolia | ArbitrumNova => 260,

            Optimism | OptimismSepolia | Base | BaseSepolia | Mantle | MantleSepolia
            | WorldChain | WorldChainSepolia => 2_000,

            Polygon | PolygonAmoy => 2_100,

            Moonbeam | Moonriver => 12_500,

            BinanceSmartChain | BinanceSmartChainTestnet => 3_000,

            Avalanche | AvalancheFuji => 2_000,

            Fantom | FantomTestnet => 1_200,

            Aurora | AuroraTestnet => 1_100,

            Celo | CeloAlfajores | CeloBaklava => 5_000,

            Scroll | ScrollSepolia => 3_000,

            Gnosis => 5_000,

            Unichain | UnichainSepolia => 1_000,

            Sepolia | Holesky | Linea | LineaSepolia => 12_000,

            Moonbase | MoonbeamDev => 10_000,

            ZkSync | ZkSyncTestnet | PolygonZkEvm | PolygonZkEvmTestnet => 5_000,
        }))
    }

    pub const fn is_legacy(self) -> bool {
        use EvmNamedNetwork::*;

        match self {
            // Known legacy chains / non EIP-1559 compliant.
            BinanceSmartChain
            | BinanceSmartChainTestnet
            | Celo
            | CeloAlfajores
            | CeloBaklava
            | Fantom
            | FantomTestnet
            | PolygonZkEvm
            | PolygonZkEvmTestnet
            | ZkSync
            | ZkSyncTestnet => true,

            // Known EIP-1559 chains.
            Mainnet | Sepolia | Holesky | Base | BaseSepolia | Optimism | OptimismSepolia
            | Polygon | PolygonAmoy | Avalanche | AvalancheFuji | Arbitrum | ArbitrumSepolia
            | ArbitrumNova | Linea | LineaSepolia | Gnosis | Mantle | MantleSepolia | Scroll
            | ScrollSepolia | Unichain | UnichainSepolia | WorldChain | WorldChainSepolia => false,

            // Unknown / not applicable, default to false for backwards compatibility.
            Moonbeam | MoonbeamDev | Moonriver | Moonbase | Aurora | AuroraTestnet => false,
        }
    }

    pub const fn is_testnet(self) -> bool {
        use EvmNamedNetwork::*;

        match self {
            Holesky
            | Sepolia
            | ArbitrumSepolia
            | AuroraTestnet
            | AvalancheFuji
            | BaseSepolia
            | BinanceSmartChainTestnet
            | CeloAlfajores
            | CeloBaklava
            | FantomTestnet
            | LineaSepolia
            | MantleSepolia
            | MoonbeamDev
            | OptimismSepolia
            | PolygonAmoy
            | PolygonZkEvmTestnet
            | ScrollSepolia
            | UnichainSepolia
            | ZkSyncTestnet
            | WorldChainSepolia => true,

            // Mainnets.
            Mainnet | Optimism | Arbitrum | ArbitrumNova | BinanceSmartChain | Scroll | Gnosis
            | Polygon | PolygonZkEvm | Fantom | Moonbeam | Moonriver | Moonbase | Avalanche
            | Celo | Aurora | Base | Linea | ZkSync | Mantle | Unichain | WorldChain => false,
        }
    }

    // TODO
    pub const fn is_deprecated(self) -> bool {
        use EvmNamedNetwork::*;

        match self {
            Holesky
            | Sepolia
            | ArbitrumSepolia
            | AuroraTestnet
            | AvalancheFuji
            | BaseSepolia
            | BinanceSmartChainTestnet
            | CeloAlfajores
            | CeloBaklava
            | FantomTestnet
            | LineaSepolia
            | MantleSepolia
            | MoonbeamDev
            | OptimismSepolia
            | PolygonAmoy
            | PolygonZkEvmTestnet
            | ScrollSepolia
            | UnichainSepolia
            | ZkSyncTestnet
            | WorldChainSepolia => true,

            // Mainnets.
            Mainnet | Optimism | Arbitrum | ArbitrumNova | BinanceSmartChain | Scroll | Gnosis
            | Polygon | PolygonZkEvm | Fantom | Moonbeam | Moonriver | Moonbase | Avalanche
            | Celo | Aurora | Base | Linea | ZkSync | Mantle | Unichain | WorldChain => false,
        }
    }

    pub const fn is_rollup(self) -> bool {
        use EvmNamedNetwork::*;

        match self {
            // Optimism-based (Bedrock) networks
            Optimism | OptimismSepolia | Base | BaseSepolia | Unichain | UnichainSepolia
            | WorldChain | WorldChainSepolia => true,

            // Arbitrum networks
            Arbitrum | ArbitrumSepolia | ArbitrumNova => true,

            // ZkSync networks
            ZkSync | ZkSyncTestnet => true,

            // Linea networks
            Linea | LineaSepolia => true,

            // Mantle networks
            Mantle | MantleSepolia => true,

            // All other networks are not rollups
            _ => false,
        }
    }

    pub const fn explorer_urls(self) -> Option<&'static [&'static str]> {
        use EvmNamedNetwork::*;

        Some(match self {
            Mainnet => &["https://api.etherscan.io/api", "https://etherscan.io"],
            Sepolia => &[
                "https://api-sepolia.etherscan.io/api",
                "https://sepolia.etherscan.io",
            ],
            Holesky => &[
                "https://api-holesky.etherscan.io/api",
                "https://holesky.etherscan.io",
            ],

            Polygon => &["https://api.polygonscan.com/api", "https://polygonscan.com"],
            PolygonAmoy => &[
                "https://api-amoy.polygonscan.com/api",
                "https://amoy.polygonscan.com",
            ],

            PolygonZkEvm => &[
                "https://api-zkevm.polygonscan.com/api",
                "https://zkevm.polygonscan.com",
            ],
            PolygonZkEvmTestnet => &[
                "https://api-testnet-zkevm.polygonscan.com/api",
                "https://testnet-zkevm.polygonscan.com",
            ],

            Avalanche => &["https://api.snowtrace.io/api", "https://snowtrace.io"],
            AvalancheFuji => &[
                "https://api-testnet.snowtrace.io/api",
                "https://testnet.snowtrace.io",
            ],

            Optimism => &[
                "https://api-optimistic.etherscan.io/api",
                "https://optimistic.etherscan.io",
            ],
            OptimismSepolia => &[
                "https://api-sepolia-optimistic.etherscan.io/api",
                "https://sepolia-optimism.etherscan.io",
            ],

            Fantom => &["https://api.ftmscan.com/api", "https://ftmscan.com"],
            FantomTestnet => &[
                "https://api-testnet.ftmscan.com/api",
                "https://testnet.ftmscan.com",
            ],

            BinanceSmartChain => &["https://api.bscscan.com/api", "https://bscscan.com"],
            BinanceSmartChainTestnet => &[
                "https://api-testnet.bscscan.com/api",
                "https://testnet.bscscan.com",
            ],

            Arbitrum => &["https://api.arbiscan.io/api", "https://arbiscan.io"],
            ArbitrumSepolia => &[
                "https://api-sepolia.arbiscan.io/api",
                "https://sepolia.arbiscan.io",
            ],
            ArbitrumNova => &[
                "https://api-nova.arbiscan.io/api",
                "https://nova.arbiscan.io",
            ],

            Moonbeam => &[
                "https://api-moonbeam.moonscan.io/api",
                "https://moonbeam.moonscan.io",
            ],
            Moonbase => &[
                "https://api-moonbase.moonscan.io/api",
                "https://moonbase.moonscan.io",
            ],
            Moonriver => &[
                "https://api-moonriver.moonscan.io/api",
                "https://moonriver.moonscan.io",
            ],

            Gnosis => &["https://api.gnosisscan.io/api", "https://gnosisscan.io"],

            Scroll => &["https://api.scrollscan.com/api", "https://scrollscan.com"],
            ScrollSepolia => &[
                "https://api-sepolia.scrollscan.com/api",
                "https://sepolia.scrollscan.com",
            ],

            Aurora => &["https://api.aurorascan.dev/api", "https://aurorascan.dev"],
            AuroraTestnet => &[
                "https://testnet.aurorascan.dev/api",
                "https://testnet.aurorascan.dev",
            ],

            Celo => &["https://api.celoscan.io/api", "https://celoscan.io"],
            CeloAlfajores => &[
                "https://api-alfajores.celoscan.io/api",
                "https://alfajores.celoscan.io",
            ],
            CeloBaklava => &[
                "https://explorer.celo.org/baklava/api",
                "https://explorer.celo.org/baklava",
            ],

            Base => &["https://api.basescan.org/api", "https://basescan.org"],
            BaseSepolia => &[
                "https://api-sepolia.basescan.org/api",
                "https://sepolia.basescan.org",
            ],

            ZkSync => &[
                "https://api-era.zksync.network/api",
                "https://era.zksync.network",
            ],
            ZkSyncTestnet => &[
                "https://api-sepolia-era.zksync.network/api",
                "https://sepolia-era.zksync.network",
            ],

            Linea => &["https://api.lineascan.build/api", "https://lineascan.build"],
            LineaSepolia => &[
                "https://api-sepolia.lineascan.build/api",
                "https://sepolia.lineascan.build",
            ],

            Mantle => &[
                "https://explorer.mantle.xyz/api",
                "https://explorer.mantle.xyz",
            ],
            MantleSepolia => &[
                "https://explorer.sepolia.mantle.xyz/api",
                "https://explorer.sepolia.mantle.xyz",
            ],

            Unichain => &["https://uniscan.xyz", "https://api.uniscan.xyz/api"],
            UnichainSepolia => &[
                "https://sepolia.uniscan.xyz",
                "https://api-sepolia.uniscan.xyz/api",
            ],
            WorldChain => &["https://worldscan.org", "https://api.worldscan.org/api"],
            WorldChainSepolia => &[
                "https://worldchain-sepolia.g.alchemy.com/public",
                "https://worldchain-sepolia.explorer.alchemy.com",
                "https://worldchain-sepolia.explorer.alchemy.com/api", // Assuming API path, might need verification
            ],
            MoonbeamDev => {
                return None;
            }
        })
    }

    pub const fn public_rpc_urls(self) -> Option<&'static [&'static str]> {
        use EvmNamedNetwork::*;

        // RPC endpoints extracted from:
        // - Official Docs
        // - https://drpc.org/chainlist
        // - https://www.1rpc.io/
        // - https://publicnode.com/
        // - https://www.nodies.app/
        // - https://chainlist.org/,
        Some(match self {
            Mainnet => &[
                "https://eth.drpc.org",
                "https://1rpc.io/eth",
                "https://ethereum-rpc.publicnode.com",
                "https://ethereum-public.nodies.app",
            ],
            Sepolia => &[
                "https://sepolia.drpc.org",
                "https://1rpc.io/sepolia",
                "https://ethereum-sepolia-rpc.publicnode.com",
                "https://ethereum-sepolia-public.nodies.app",
            ],
            Holesky => &[
                "https://holesky.drpc.org",
                "https://1rpc.io/holesky",
                "https://ethereum-holesky-rpc.publicnode.com",
            ],

            Polygon => &[
                "https://polygon-rpc.com",
                "https://polygon.drpc.org",
                "https://1rpc.io/matic",
                "https://polygon-bor-rpc.publicnode.com",
                "https://polygon-public.nodies.app",
                "https://polygon.meowrpc.com",
            ],
            PolygonAmoy => &[
                "https://rpc-amoy.polygon.technology",
                "https://polygon-amoy.drpc.org",
                "https://polygon-amoy-bor-rpc.publicnode.com",
            ],

            PolygonZkEvm => &[
                "https://zkevm-rpc.com",
                "https://polygon-zkevm.drpc.org",
                "https://1rpc.io/polygon/zkevm",
                "https://polygon-zkevm-public.nodies.app",
            ],
            PolygonZkEvmTestnet => &[
                "https://rpc.cardona.zkevm-rpc.com",
                "https://polygon-zkevm-cardona.drpc.org",
                "https://testnet-zkevm.polygonscan.com",
            ],

            Avalanche => &[
                "https://avalanche.drpc.org",
                "https://1rpc.io/avax/c",
                "https://avalanche-public.nodies.app",
            ],
            AvalancheFuji => &[
                "https://avalanche-fuji.drpc.org",
                "https://avalanche-fuji-c-chain-rpc.publicnode.com",
            ],

            Optimism => &[
                "https://mainnet.optimism.io",
                "https://optimism.drpc.org",
                "https://1rpc.io/op",
                "https://optimism-rpc.publicnode.com",
                "https://optimism-public.nodies.app",
            ],
            OptimismSepolia => &[
                "https://sepolia.optimism.io",
                "https://optimism-sepolia.drpc.org",
                "https://optimism-sepolia-rpc.publicnode.com",
                "https://optimism-sepolia-public.nodies.app",
            ],

            Fantom => &[
                "https://rpcapi.fantom.network",
                "https://fantom.drpc.org",
                "https://1rpc.io/ftm",
                "https://fantom-rpc.publicnode.com",
                "https://fantom-public.nodies.app",
            ],
            FantomTestnet => &[
                "https://rpc.testnet.fantom.network/",
                "https://fantom-testnet.drpc.org",
                "https://fantom-testnet-rpc.publicnode.com",
            ],

            // Binance Smart Chain RPC extracted from https://docs.bnbchain.org/bnb-smart-chain/developers/json_rpc/json-rpc-endpoint
            BinanceSmartChain => &[
                "https://bsc-dataseed.bnbchain.org",
                "https://bsc-dataseed.nariox.org",
                "https://bsc-dataseed.defibit.io",
                "https://bsc-dataseed.ninicoin.io",
                "https://bsc.nodereal.io",
                "https://bsc-dataseed-public.bnbchain.org",
                "https://bnb.rpc.subquery.network/public",
                "https://bsc.drpc.org",
                "https://1rpc.io/bnb",
                "https://bsc-rpc.publicnode.com",
                "https://binance-smart-chain-public.nodies.app",
            ],
            BinanceSmartChainTestnet => &[
                "https://bsc-testnet-dataseed.bnbchain.org",
                "https://bsc-testnet.bnbchain.org",
                "https://bsc-prebsc-dataseed.bnbchain.org",
                "https://bsc-testnet.drpc.org",
                "https://bsc-testnet-rpc.publicnode.com",
            ],

            Arbitrum => &[
                "https://arb1.arbitrum.io/rpc",
                "https://arbitrum.drpc.org",
                "https://1rpc.io/arb",
                "https://arbitrum-one-rpc.publicnode.com",
                "https://arbitrum-one-public.nodies.app",
            ],
            ArbitrumSepolia => &[
                "https://sepolia-rollup.arbitrum.io/rpc",
                "https://arbitrum-sepolia.drpc.org",
                "https://arbitrum-sepolia-rpc.publicnode.com",
            ],
            ArbitrumNova => &[
                "https://nova.arbitrum.io/rpc",
                "https://arbitrum-nova.drpc.org",
                "https://arbitrum-nova-rpc.publicnode.com",
            ],

            Moonbeam => &[
                "https://api-moonbeam.moonscan.io/api",
                "https://moonbeam.moonscan.io",
            ],
            // Moonbase and Moonriver RPC extracted from https://docs.moonbeam.network/learn/platform/networks/moonbase/
            Moonbase => &[
                "https://moonbase-alpha.public.blastapi.io",
                "https://moonbase-rpc.dwellir.com",
                "https://moonbeam-alpha.api.onfinality.io/public",
                "https://rpc.api.moonbase.moonbeam.network",
                "https://moonbase.unitedbloc.com",
                "https://moonbase.public.curie.radiumblock.co/http",
            ],
            Moonriver => &[
                "https://moonriver.public.blastapi.io",
                "https://moonriver-rpc.dwellir.com",
                "https://moonriver.api.onfinality.io/public",
                "https://moonriver.unitedbloc.com",
                "https://moonriver.public.curie.radiumblock.co/http",
            ],

            // Gnosis RPC extracted from https://docs.gnosischain.com/tools/RPC%20Providers/
            Gnosis => &[
                "https://rpc.gnosis.gateway.fm",
                "https://rpc.gnosischain.com/",
                "https://rpc.ankr.com/gnosis",
                "https://gnosis-pokt.nodies.app",
                "https://gnosis-mainnet.public.blastapi.io",
                "https://gnosis.api.onfinality.io/public",
            ],

            Scroll => &[
                "https://rpc.scroll.io/",
                "https://scroll.drpc.org",
                "https://1rpc.io/scroll",
                "https://scroll-rpc.publicnode.com",
                "https://scroll-public.nodies.app",
            ],
            ScrollSepolia => &[
                "https://sepolia-rpc.scroll.io/",
                "https://scroll-sepolia.drpc.org",
                "https://scroll-sepolia-rpc.publicnode.com",
                "https://scroll-sepolia-public.nodies.app",
            ],

            Aurora => &[
                "https://mainnet.aurora.dev",
                "https://aurora.drpc.org",
                "https://1rpc.io/aurora",
            ],
            AuroraTestnet => &[
                "https://testnet.aurora.dev",
                "https://aurora-testnet.drpc.org",
            ],

            Celo => &[
                "https://rpc.ankr.com/celo",
                "https://celo.drpc.org",
                "https://1rpc.io/celo",
                "https://celo-rpc.publicnode.com",
            ],
            CeloAlfajores => &[
                "https://celo-alfajores.drpc.org",
                "https://alfajores-forno.celo-testnet.org",
            ],
            CeloBaklava => &["https://baklava-forno.celo-testnet.org"],

            Base => &[
                "https://mainnet.base.org",
                "https://base.drpc.org",
                "https://1rpc.io/base",
                "https://base-rpc.publicnode.com",
                "https://base-public.nodies.app",
            ],
            BaseSepolia => &[
                "https://sepolia.base.org",
                "https://base-sepolia.drpc.org",
                "https://base-sepolia-rpc.publicnode.com",
                "https://base-sepolia-public.nodies.app",
            ],

            ZkSync => &[
                "https://mainnet.era.zksync.io",
                "https://zksync.drpc.org",
                "https://1rpc.io/zksync2-era",
            ],
            ZkSyncTestnet => &[
                "https://sepolia.era.zksync.dev",
                "https://zksync-sepolia.drpc.org",
            ],

            Linea => &[
                "https://rpc.linea.build",
                "https://linea.drpc.org",
                "https://1rpc.io/linea",
                "https://linea-rpc.publicnode.com",
            ],
            LineaSepolia => &[
                "https://rpc.sepolia.linea.build",
                "https://linea-sepolia.drpc.org",
                "https://linea-sepolia-rpc.publicnode.com",
            ],

            Mantle => &[
                "https://rpc.mantle.xyz",
                "https://mantle.drpc.org",
                "https://1rpc.io/mantle",
                "https://mantle-rpc.publicnode.com",
                "https://mantle-public.nodies.app",
            ],
            MantleSepolia => &[
                "https://rpc.sepolia.mantle.xyz",
                "https://mantle-sepolia.drpc.org",
            ],

            Unichain => &[
                "https://mainnet.unichain.org",
                "https://unichain.drpc.org",
                "https://1rpc.io/unichain",
                "https://unichain-rpc.publicnode.com",
            ],
            UnichainSepolia => &[
                "https://sepolia.unichain.org",
                "https://unichain-sepolia.drpc.org",
                "https://unichain-sepolia-rpc.publicnode.com",
            ],
            WorldChain => &[
                "https://worldchain-mainnet.g.alchemy.com/public",
                "https://worldchain.drpc.org",
            ],
            WorldChainSepolia => &[
                "https://worldchain-sepolia.g.alchemy.com/public",
                "https://worldchain-sepolia.drpc.org",
            ],
            MoonbeamDev => {
                return None;
            }
        })
    }

    pub const fn native_currency_symbol(self) -> &'static str {
        use EvmNamedNetwork::*;
        match self {
            // Ethereum and L2s
            Mainnet | Sepolia | Holesky | Optimism | OptimismSepolia | Base | BaseSepolia
            | Arbitrum | ArbitrumSepolia | ArbitrumNova | Scroll | ScrollSepolia | ZkSync
            | ZkSyncTestnet => "ETH",
            Celo | CeloAlfajores | CeloBaklava => "CELO",
            Mantle | MantleSepolia => "MNT",
            Linea | LineaSepolia => "ETH",

            // BSC
            BinanceSmartChain | BinanceSmartChainTestnet => "BNB",

            // Polygon
            Polygon | PolygonAmoy => "POL",
            PolygonZkEvm | PolygonZkEvmTestnet => "ETH",

            // L1s
            Fantom | FantomTestnet => "FTM",
            Moonbeam | MoonbeamDev => "GLMR",
            Moonriver => "MOVR",
            Moonbase => "DEV",
            Avalanche | AvalancheFuji => "AVAX",
            Gnosis => "xDAI",
            Unichain | UnichainSepolia => "ETH",

            WorldChain | WorldChainSepolia => "ETH",
            Aurora | AuroraTestnet => "ETH",
        }
    }

    /// Returns the recommended number of confirmations needed for each network.
    pub const fn required_confirmations(self) -> u64 {
        use EvmNamedNetwork::*;

        match self {
            Mainnet => 12,
            Sepolia | Holesky => 6,
            // TODO: Add more networks
            _ => 1,
        }
    }
}

impl fmt::Display for EvmNamedNetwork {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_str().fmt(f)
    }
}

impl AsRef<str> for EvmNamedNetwork {
    #[inline]
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl serde::Serialize for EvmNamedNetwork {
    #[inline]
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use strum::IntoEnumIterator;
    #[allow(unused_imports)]
    #[test]
    fn default() {
        assert_eq!(
            serde_json::to_string(&EvmNamedNetwork::default()).unwrap(),
            "\"mainnet\""
        );
    }

    #[test]
    fn roundtrip_string() {
        for chain in EvmNamedNetwork::iter() {
            let chain_string = chain.to_string();
            assert_eq!(chain_string, format!("{chain}"));
            assert_eq!(chain_string.as_str(), chain.as_ref());
            assert_eq!(
                serde_json::to_string(&chain).unwrap(),
                format!("\"{chain_string}\"")
            );

            assert_eq!(chain_string.parse::<EvmNamedNetwork>().unwrap(), chain);
        }
    }

    #[test]
    fn is_testnet() {
        assert!(!EvmNamedNetwork::Mainnet.is_testnet());
        assert!(EvmNamedNetwork::Sepolia.is_testnet());
        assert!(EvmNamedNetwork::UnichainSepolia.is_testnet());
        assert!(EvmNamedNetwork::WorldChainSepolia.is_testnet());
    }

    #[test]
    fn is_ethereum() {
        assert!(EvmNamedNetwork::Mainnet.is_ethereum());
        assert!(EvmNamedNetwork::Sepolia.is_ethereum());
        assert!(!EvmNamedNetwork::Arbitrum.is_ethereum());
    }

    #[test]
    fn roundtrip_serde() {
        for chain in EvmNamedNetwork::iter() {
            let chain_string = serde_json::to_string(&chain).unwrap();
            let chain_string = chain_string.replace('-', "_");
            assert_eq!(
                serde_json::from_str::<'_, EvmNamedNetwork>(&chain_string).unwrap(),
                chain
            );
        }
    }

    #[test]
    fn symbol() {
        assert_eq!(EvmNamedNetwork::Mainnet.native_currency_symbol(), "ETH");
        assert_eq!(EvmNamedNetwork::Sepolia.native_currency_symbol(), "ETH");
        assert_eq!(
            EvmNamedNetwork::WorldChainSepolia.native_currency_symbol(),
            "ETH"
        );
        assert_eq!(
            EvmNamedNetwork::BinanceSmartChain.native_currency_symbol(),
            "BNB"
        );
        assert_eq!(EvmNamedNetwork::Polygon.native_currency_symbol(), "POL");
    }

    #[test]
    fn is_optimism_check() {
        for net in [
            EvmNamedNetwork::Optimism,
            EvmNamedNetwork::OptimismSepolia,
            EvmNamedNetwork::Base,
            EvmNamedNetwork::BaseSepolia,
            EvmNamedNetwork::UnichainSepolia,
            EvmNamedNetwork::Unichain,
            EvmNamedNetwork::WorldChain,
            EvmNamedNetwork::WorldChainSepolia,
        ] {
            assert!(net.is_optimism());
        }
        assert!(!EvmNamedNetwork::Mainnet.is_optimism());
        assert!(!EvmNamedNetwork::Arbitrum.is_optimism());
    }

    #[test]
    fn is_arbitrum_check() {
        for net in [
            EvmNamedNetwork::Arbitrum,
            EvmNamedNetwork::ArbitrumSepolia,
            EvmNamedNetwork::ArbitrumNova,
        ] {
            assert!(net.is_arbitrum());
        }
        assert!(!EvmNamedNetwork::Mainnet.is_arbitrum());
        assert!(!EvmNamedNetwork::Optimism.is_arbitrum());
    }

    #[test]
    fn average_blocktime_values() {
        assert_eq!(
            EvmNamedNetwork::Mainnet.average_blocktime(),
            Some(Duration::from_millis(12000))
        );
        assert_eq!(
            EvmNamedNetwork::Optimism.average_blocktime(),
            Some(Duration::from_millis(2000))
        );
        assert_eq!(
            EvmNamedNetwork::WorldChain.average_blocktime(),
            Some(Duration::from_millis(2000))
        );
        assert_eq!(
            EvmNamedNetwork::WorldChainSepolia.average_blocktime(),
            Some(Duration::from_millis(2000))
        );
    }

    #[test]
    fn is_legacy_check() {
        assert!(EvmNamedNetwork::BinanceSmartChain.is_legacy());
        assert!(EvmNamedNetwork::Celo.is_legacy());
        assert!(!EvmNamedNetwork::Mainnet.is_legacy());
        assert!(!EvmNamedNetwork::Polygon.is_legacy());
        assert!(!EvmNamedNetwork::WorldChainSepolia.is_legacy());
        assert!(!EvmNamedNetwork::WorldChain.is_legacy());
    }

    #[test]
    fn is_deprecated_check() {
        assert!(EvmNamedNetwork::Sepolia.is_deprecated());
        assert!(EvmNamedNetwork::WorldChainSepolia.is_deprecated());
        assert!(!EvmNamedNetwork::Mainnet.is_deprecated());
        assert!(!EvmNamedNetwork::Optimism.is_deprecated());
    }

    #[test]
    fn explorer_urls_check() {
        let mainnet = EvmNamedNetwork::Mainnet.explorer_urls().unwrap();
        assert!(mainnet.contains(&"https://api.etherscan.io/api"));
        assert!(mainnet.contains(&"https://etherscan.io"));
        let wc_sep = EvmNamedNetwork::WorldChainSepolia.explorer_urls().unwrap();
        assert!(wc_sep.contains(&"https://worldchain-sepolia.explorer.alchemy.com"));
        assert_eq!(EvmNamedNetwork::MoonbeamDev.explorer_urls(), None);
    }

    #[test]
    fn public_rpc_urls_check() {
        assert!(EvmNamedNetwork::Sepolia
            .public_rpc_urls()
            .unwrap()
            .contains(&"https://sepolia.drpc.org"));
        assert!(EvmNamedNetwork::WorldChainSepolia
            .public_rpc_urls()
            .unwrap()
            .contains(&"https://worldchain-sepolia.g.alchemy.com/public"));
        assert_eq!(EvmNamedNetwork::MoonbeamDev.public_rpc_urls(), None);
    }

    #[test]
    fn is_rollup_check() {
        // Test Optimism-based networks
        for net in [
            EvmNamedNetwork::Optimism,
            EvmNamedNetwork::OptimismSepolia,
            EvmNamedNetwork::Base,
            EvmNamedNetwork::BaseSepolia,
            EvmNamedNetwork::Unichain,
            EvmNamedNetwork::UnichainSepolia,
            EvmNamedNetwork::WorldChain,
            EvmNamedNetwork::WorldChainSepolia,
        ] {
            assert!(net.is_rollup(), "{} should be a rollup", net);
        }

        // Test Arbitrum networks
        for net in [
            EvmNamedNetwork::Arbitrum,
            EvmNamedNetwork::ArbitrumSepolia,
            EvmNamedNetwork::ArbitrumNova,
        ] {
            assert!(net.is_rollup(), "{} should be a rollup", net);
        }

        // Test ZkSync networks
        for net in [EvmNamedNetwork::ZkSync, EvmNamedNetwork::ZkSyncTestnet] {
            assert!(net.is_rollup(), "{} should be a rollup", net);
        }

        // Test Linea networks
        for net in [EvmNamedNetwork::Linea, EvmNamedNetwork::LineaSepolia] {
            assert!(net.is_rollup(), "{} should be a rollup", net);
        }

        // Test Mantle networks
        for net in [EvmNamedNetwork::Mantle, EvmNamedNetwork::MantleSepolia] {
            assert!(net.is_rollup(), "{} should be a rollup", net);
        }

        // Test non-rollup networks
        for net in [
            EvmNamedNetwork::Mainnet,
            EvmNamedNetwork::Sepolia,
            EvmNamedNetwork::BinanceSmartChain,
            EvmNamedNetwork::Polygon,
            EvmNamedNetwork::Avalanche,
            EvmNamedNetwork::Fantom,
            EvmNamedNetwork::Moonbeam,
            EvmNamedNetwork::Celo,
            EvmNamedNetwork::Aurora,
        ] {
            assert!(!net.is_rollup(), "{} should not be a rollup", net);
        }
    }
}
