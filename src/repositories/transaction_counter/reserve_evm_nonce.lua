-- Counter, authoritative transaction nonce and discovery index have one commit boundary.
-- All validation reads precede all writes; JSON and integers are serialized exactly by Rust.
if redis.call('GET', KEYS[3]) ~= ARGV[1] then return false end
if redis.call('GET', KEYS[2]) ~= ARGV[2] then return false end
local counter = redis.call('GET', KEYS[1])
if (counter or '') ~= ARGV[3] then return false end
local owner = redis.call('GET', KEYS[4])
if owner and owner ~= ARGV[6] then return redis.error_reply('EVM nonce slot already occupied') end
redis.call('SET', KEYS[1], ARGV[4])
redis.call('SET', KEYS[2], ARGV[5])
redis.call('SET', KEYS[4], ARGV[6])
return true
