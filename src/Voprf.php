<?php

/**
 * Server half of the VOPRF that pseudonymises patients.
 *
 * RFC 9497, ciphersuite ristretto255-SHA512, VOPRF mode, batch size 1. REDCap
 * evaluates the browser's blinded element with its key and proves, with a
 * DLEQ proof, that it used the key whose public half it publishes. The wire
 * format is in protocol/README.md; the Rust reference is
 * crates/liryc-pseudonym, and both must agree byte for byte.
 *
 * Every value crosses this API as lowercase hex: elements and scalars are 32
 * bytes (64 hex characters), a proof is c ‖ s (128 hex characters).
 *
 * Only ext-sodium is needed. Its ristretto255 scalar multiplications fail when
 * the result is the identity, which happens exactly when the scalar is zero or
 * the point is the identity (the group has prime order); both cases are taken
 * out before calling it, see multiply().
 */

declare(strict_types=1);

namespace ExternalModules\GirderUploaderModule;

final class Voprf
{
    /** `"OPRFV1-" ‖ I2OSP(mode = VOPRF, 1) ‖ "-" ‖ identifier`. */
    private const CONTEXT_STRING = "OPRFV1-\x01-ristretto255-SHA512";

    private const BYTES = 32;

    /** Encoding of the identity element, and of the zero scalar. */
    private const ZERO = "\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0";

    private function __construct()
    {
    }

    /** Whether this PHP has what the protocol needs: ristretto255 came with PHP 8.1. */
    public static function isAvailable(): bool
    {
        return function_exists('sodium_crypto_core_ristretto255_scalar_random')
            && function_exists('sodium_crypto_scalarmult_ristretto255');
    }

    // --- Keys ---------------------------------------------------------------

    /** A uniformly random non-zero scalar, as hex. */
    public static function generateKey(): string
    {
        do {
            $key = sodium_crypto_core_ristretto255_scalar_random();
        } while (!self::isCanonicalNonZeroScalar($key));

        return bin2hex($key);
    }

    /** 32 bytes of hex encoding a canonical, non-zero scalar. */
    public static function isValidKey(string $hex): bool
    {
        $key = self::decodeHex($hex, self::BYTES);

        return $key !== null && self::isCanonicalNonZeroScalar($key);
    }

    /** `K = k·G`, as hex. */
    public static function publicKey(string $keyHex): string
    {
        return bin2hex(sodium_crypto_scalarmult_ristretto255_base(self::key($keyHex)));
    }

    // --- Evaluation ---------------------------------------------------------

    /**
     * RFC 9497 `BlindEvaluate` with its proof.
     *
     * The proof randomness is drawn fresh unless given; only the test vectors
     * give it.
     *
     * @return array{evaluatedElement: string, proof: string, publicKey: string}
     * @throws \InvalidArgumentException for an invalid key, blinded element or
     *         proof randomness.
     */
    public static function blindEvaluate(string $keyHex, string $blindedElementHex, ?string $proofRandomScalarHex = null): array
    {
        $key = self::key($keyHex);
        $blinded = self::element($blindedElementHex);
        if ($blinded === null) {
            throw new \InvalidArgumentException('Invalid blinded element: expected a 32-byte ristretto255 point other than the identity, as hex.');
        }

        if ($proofRandomScalarHex === null) {
            $random = self::randomScalar();
        } else {
            $random = self::decodeHex($proofRandomScalarHex, self::BYTES);
            // A zero r would publish s = -c·k, and with it the key.
            if ($random === null || !self::isCanonicalNonZeroScalar($random)) {
                throw new \InvalidArgumentException('Invalid proof randomness: expected a canonical non-zero scalar, as hex.');
            }
        }

        $publicKey = sodium_crypto_scalarmult_ristretto255_base($key);
        $evaluated = sodium_crypto_scalarmult_ristretto255($key, $blinded);
        $proof = self::generateProof($key, $publicKey, $blinded, $evaluated, $random);

        return [
            'evaluatedElement' => bin2hex($evaluated),
            'proof' => bin2hex($proof),
            'publicKey' => bin2hex($publicKey),
        ];
    }

    /**
     * RFC 9497 `VerifyProof` for a batch of one, with the generator as `A`:
     * whoever holds the secret half of `publicKey` turned `blinded` into
     * `evaluated`. Malformed input of any kind yields false.
     */
    public static function verifyProof(string $publicKeyHex, string $blindedHex, string $evaluatedHex, string $proofHex): bool
    {
        $publicKey = self::element($publicKeyHex);
        $blinded = self::element($blindedHex);
        $evaluated = self::element($evaluatedHex);
        $proof = self::decodeHex($proofHex, 2 * self::BYTES);
        if ($publicKey === null || $blinded === null || $evaluated === null || $proof === null) {
            return false;
        }

        $c = substr($proof, 0, self::BYTES);
        $s = substr($proof, self::BYTES);
        if (!self::isCanonicalScalar($c) || !self::isCanonicalScalar($s)) {
            return false;
        }

        [$m, $z] = self::computeComposites($publicKey, $blinded, $evaluated);
        $t2 = self::add(self::multiplyBase($s), self::multiply($c, $publicKey));
        $t3 = self::add(self::multiply($s, $m), self::multiply($c, $z));

        return hash_equals(self::challenge($publicKey, $m, $z, $t2, $t3), $c);
    }

    // --- Proofs -------------------------------------------------------------

    /** RFC 9497 `GenerateProof`; returns `c ‖ s`. */
    private static function generateProof(string $key, string $publicKey, string $blinded, string $evaluated, string $random): string
    {
        [$m] = self::computeComposites($publicKey, $blinded, $evaluated);
        $z = self::multiply($key, $m);
        $t2 = self::multiplyBase($random);
        $t3 = self::multiply($random, $m);
        $c = self::challenge($publicKey, $m, $z, $t2, $t3);
        $s = sodium_crypto_core_ristretto255_scalar_sub($random, sodium_crypto_core_ristretto255_scalar_mul($c, $key));

        return $c . $s;
    }

    /**
     * `ComputeComposites` for one element pair: `[d·C, d·D]`. The server's fast
     * variant computes `Z = k·M` instead, the same point for a single pair.
     *
     * @return array{0: string, 1: string}
     */
    private static function computeComposites(string $publicKey, string $c, string $d): array
    {
        $seedDst = 'Seed-' . self::CONTEXT_STRING;
        $seed = hash('sha512', self::i2osp2(self::BYTES) . $publicKey . self::i2osp2(strlen($seedDst)) . $seedDst, true);

        $transcript = self::i2osp2(strlen($seed)) . $seed
            . self::i2osp2(0)
            . self::i2osp2(self::BYTES) . $c
            . self::i2osp2(self::BYTES) . $d
            . 'Composite';
        $di = self::hashToScalar($transcript);

        return [self::multiply($di, $c), self::multiply($di, $d)];
    }

    private static function challenge(string $publicKey, string $m, string $z, string $t2, string $t3): string
    {
        $transcript = '';
        foreach ([$publicKey, $m, $z, $t2, $t3] as $point) {
            $transcript .= self::i2osp2(self::BYTES) . $point;
        }

        return self::hashToScalar($transcript . 'Challenge');
    }

    // --- Hashing ------------------------------------------------------------

    private static function hashToScalar(string $input): string
    {
        return sodium_crypto_core_ristretto255_scalar_reduce(
            self::expandMessageXmd($input, 'HashToScalar-' . self::CONTEXT_STRING)
        );
    }

    /**
     * RFC 9380 §5.3.1 with SHA-512, for an output of exactly one block (64
     * bytes), which is all the ciphersuite asks for.
     */
    private static function expandMessageXmd(string $message, string $dst): string
    {
        $dstPrime = $dst . chr(strlen($dst));
        $b0 = hash('sha512', str_repeat("\0", 128) . $message . self::i2osp2(64) . "\0" . $dstPrime, true);

        return hash('sha512', $b0 . "\x01" . $dstPrime, true);
    }

    // --- Group --------------------------------------------------------------

    /**
     * `n·P`. libsodium refuses to return the identity, which comes out exactly
     * when `n` is zero or `P` is the identity; both are answered here.
     */
    private static function multiply(string $scalar, string $point): string
    {
        if (hash_equals(self::ZERO, $scalar) || hash_equals(self::ZERO, $point)) {
            return self::ZERO;
        }

        return sodium_crypto_scalarmult_ristretto255($scalar, $point);
    }

    /** `n·G`, with the same treatment of zero as multiply(). */
    private static function multiplyBase(string $scalar): string
    {
        if (hash_equals(self::ZERO, $scalar)) {
            return self::ZERO;
        }

        return sodium_crypto_scalarmult_ristretto255_base($scalar);
    }

    private static function add(string $p, string $q): string
    {
        return sodium_crypto_core_ristretto255_add($p, $q);
    }

    // --- Encoding -----------------------------------------------------------

    /** The binary key, or an exception. */
    private static function key(string $keyHex): string
    {
        $key = self::decodeHex($keyHex, self::BYTES);
        if ($key === null || !self::isCanonicalNonZeroScalar($key)) {
            throw new \InvalidArgumentException('Invalid key: expected a canonical non-zero 32-byte scalar, as hex.');
        }

        return $key;
    }

    /** A valid ristretto255 element other than the identity, or null. */
    private static function element(string $hex): ?string
    {
        $bytes = self::decodeHex($hex, self::BYTES);
        if ($bytes === null || hash_equals(self::ZERO, $bytes)) {
            return null;
        }

        // libsodium 1.0.18 masks the top bit before decoding, so it accepts two
        // encodings of some points; RFC 9496 allows only the one with it clear.
        if ((ord($bytes[31]) & 0x80) !== 0) {
            return null;
        }
        return sodium_crypto_core_ristretto255_is_valid_point($bytes) ? $bytes : null;
    }

    /** Exactly `$length` bytes of hex, either case, or null. */
    private static function decodeHex(string $hex, int $length): ?string
    {
        $hex = trim($hex);
        if (strlen($hex) !== 2 * $length || preg_match('/\A[0-9a-fA-F]+\z/', $hex) !== 1) {
            return null;
        }

        try {
            return sodium_hex2bin($hex);
        } catch (\SodiumException $error) {
            return null;
        }
    }

    /** Below the group order: reducing it leaves it unchanged. */
    private static function isCanonicalScalar(string $scalar): bool
    {
        return strlen($scalar) === self::BYTES
            && hash_equals($scalar, sodium_crypto_core_ristretto255_scalar_reduce($scalar . self::ZERO));
    }

    private static function isCanonicalNonZeroScalar(string $scalar): bool
    {
        return self::isCanonicalScalar($scalar) && !hash_equals(self::ZERO, $scalar);
    }

    private static function randomScalar(): string
    {
        do {
            $scalar = sodium_crypto_core_ristretto255_scalar_random();
        } while (!self::isCanonicalNonZeroScalar($scalar));

        return $scalar;
    }

    private static function i2osp2(int $n): string
    {
        return pack('n', $n);
    }
}
