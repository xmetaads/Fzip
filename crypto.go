// ZIP encryption: legacy ZipCrypto plus WinZip AES (AE-1 / AE-2).

package main

import (
	"crypto/aes"
	"crypto/cipher"
	"crypto/hmac"
	"crypto/pbkdf2"
	"crypto/rand"
	"crypto/sha1"
	"encoding/binary"
	"errors"
	"fmt"
	"hash/crc32"
	"io"
)

var (
	errTooShort      = errors.New("encrypted data too short")
	errWrongPassword = errors.New("wrong password")
	// The HMAC did not match: the data was modified or is corrupt.
	errTampered = errors.New("authentication failed")
)

// ---------------- ZipCrypto (legacy PKWARE) ----------------

type zipCrypto struct{ k0, k1, k2 uint32 }

func newZipCrypto(password []byte) *zipCrypto {
	z := &zipCrypto{k0: 0x12345678, k1: 0x23456789, k2: 0x34567890}
	for _, b := range password {
		z.update(b)
	}
	return z
}

func crc32Byte(crc uint32, b byte) uint32 {
	return crc>>8 ^ crc32.IEEETable[(crc^uint32(b))&0xFF]
}

func (z *zipCrypto) update(b byte) {
	z.k0 = crc32Byte(z.k0, b)
	z.k1 = (z.k1+z.k0&0xFF)*134775813 + 1
	z.k2 = crc32Byte(z.k2, byte(z.k1>>24))
}

func (z *zipCrypto) decryptByte(c byte) byte {
	t := uint16(z.k2|3) & 0xFFFF
	streamByte := byte((t * (t ^ 1)) >> 8)
	p := c ^ streamByte
	z.update(p)
	return p
}

// zipCryptoDecrypt decrypts a ZipCrypto entry, returning the still-compressed
// payload. check is the expected verification byte.
func zipCryptoDecrypt(raw []byte, password string, check byte) ([]byte, error) {
	if len(raw) < 12 {
		return nil, errTooShort
	}
	z := newZipCrypto([]byte(password))
	var header [12]byte
	for i, c := range raw[:12] {
		header[i] = z.decryptByte(c)
	}
	if header[11] != check {
		return nil, errWrongPassword
	}
	out := make([]byte, 0, len(raw)-12)
	for _, c := range raw[12:] {
		out = append(out, z.decryptByte(c))
	}
	return out, nil
}

// ---------------- WinZip AES (AE-1 / AE-2) ----------------

// 1 = AES-128, 2 = AES-192, 3 = AES-256
func aesSaltLen(strength byte) int {
	switch strength {
	case 1:
		return 8
	case 2:
		return 12
	default:
		return 16
	}
}

func aesKeyLen(strength byte) int {
	switch strength {
	case 1:
		return 16
	case 2:
		return 24
	default:
		return 32
	}
}

// ctrStream is WinZip-flavoured AES-CTR: a 128-bit LITTLE-endian counter
// starting at 1. Go's cipher.NewCTR increments big-endian, so it cannot be used
// here. Keeping the keystream position across calls lets a file be encrypted
// chunk by chunk without ever holding it whole.
type ctrStream struct {
	block   cipher.Block
	counter uint64
	ks      [16]byte
	pos     int
}

func newCtrStream(key []byte) (*ctrStream, error) {
	b, err := aes.NewCipher(key)
	if err != nil {
		return nil, err
	}
	// pos == 16 forces a fresh keystream block on first use.
	return &ctrStream{block: b, counter: 1, pos: 16}, nil
}

func (c *ctrStream) refill() {
	var in [16]byte
	binary.LittleEndian.PutUint64(in[:8], c.counter)
	// The high 8 bytes stay zero: the counter never reaches 2^64.
	c.block.Encrypt(c.ks[:], in[:])
	c.counter++
	c.pos = 0
}

func (c *ctrStream) apply(data []byte) {
	for i := 0; i < len(data); {
		if c.pos == 16 {
			c.refill()
		}
		n := 16 - c.pos
		if r := len(data) - i; n > r {
			n = r
		}
		for k := 0; k < n; k++ {
			data[i+k] ^= c.ks[c.pos+k]
		}
		c.pos += n
		i += n
	}
}

type derivedKeys struct {
	enc      []byte
	auth     []byte
	verifier [2]byte
}

func derive(password string, salt []byte, keyLen int) (derivedKeys, error) {
	out, err := pbkdf2.Key(sha1.New, password, salt, 1000, keyLen*2+2)
	if err != nil {
		return derivedKeys{}, err
	}
	k := derivedKeys{enc: out[:keyLen], auth: out[keyLen : keyLen*2]}
	k.verifier[0] = out[keyLen*2]
	k.verifier[1] = out[keyLen*2+1]
	return k, nil
}

// aesDecrypt decrypts an entry laid out as salt | verifier(2) | ciphertext | mac(10).
func aesDecrypt(raw []byte, strength byte, password string) ([]byte, error) {
	saltLen := aesSaltLen(strength)
	keyLen := aesKeyLen(strength)
	if len(raw) < saltLen+2+10 {
		return nil, errTooShort
	}
	salt := raw[:saltLen]
	verifier := raw[saltLen : saltLen+2]
	ciphertext := raw[saltLen+2 : len(raw)-10]
	authCode := raw[len(raw)-10:]

	keys, err := derive(password, salt, keyLen)
	if err != nil {
		return nil, err
	}
	if !hmac.Equal(keys.verifier[:], verifier) {
		return nil, errWrongPassword
	}

	// Verify BEFORE decrypting (encrypt-then-MAC) so tampered data never
	// reaches the decryption path.
	mac := hmac.New(sha1.New, keys.auth)
	mac.Write(ciphertext)
	if !hmac.Equal(mac.Sum(nil)[:10], authCode) {
		return nil, errTampered
	}

	ctr, err := newCtrStream(keys.enc)
	if err != nil {
		return nil, err
	}
	plain := make([]byte, len(ciphertext))
	copy(plain, ciphertext)
	ctr.apply(plain)
	return plain, nil
}

// aesEncrypt encrypts one entry with AES-256 (AE-2), producing
// salt | verifier | ciphertext | mac.
func aesEncrypt(plain []byte, password string) ([]byte, error) {
	const strength = 3 // AES-256
	saltLen := aesSaltLen(strength)
	keyLen := aesKeyLen(strength)

	salt := make([]byte, saltLen)
	if _, err := rand.Read(salt); err != nil {
		return nil, fmt.Errorf("cannot obtain random bytes: %w", err)
	}
	keys, err := derive(password, salt, keyLen)
	if err != nil {
		return nil, err
	}

	body := make([]byte, len(plain))
	copy(body, plain)
	ctr, err := newCtrStream(keys.enc)
	if err != nil {
		return nil, err
	}
	ctr.apply(body)

	mac := hmac.New(sha1.New, keys.auth)
	mac.Write(body)
	tag := mac.Sum(nil)

	out := make([]byte, 0, saltLen+2+len(body)+10)
	out = append(out, salt...)
	out = append(out, keys.verifier[:]...)
	out = append(out, body...)
	out = append(out, tag[:10]...)
	return out, nil
}

// aesWriter is a streaming WinZip-AES encryptor. It writes salt|verifier on
// creation, encrypts and authenticates each chunk as it passes through, and
// appends the 10-byte MAC on Finish.
type aesWriter struct {
	inner   io.Writer
	ctr     *ctrStream
	mac     hash
	buf     []byte
	written uint64
}

// hash is the subset of hash.Hash this file needs, named locally so the import
// list stays honest about what is used.
type hash interface {
	io.Writer
	Sum(b []byte) []byte
}

func newAesWriter(inner io.Writer, password string) (*aesWriter, error) {
	const strength = 3 // AES-256
	salt := make([]byte, aesSaltLen(strength))
	if _, err := rand.Read(salt); err != nil {
		return nil, fmt.Errorf("cannot obtain random bytes: %w", err)
	}
	keys, err := derive(password, salt, aesKeyLen(strength))
	if err != nil {
		return nil, err
	}
	if _, err := inner.Write(salt); err != nil {
		return nil, fmt.Errorf("write error: %w", err)
	}
	if _, err := inner.Write(keys.verifier[:]); err != nil {
		return nil, fmt.Errorf("write error: %w", err)
	}
	ctr, err := newCtrStream(keys.enc)
	if err != nil {
		return nil, err
	}
	return &aesWriter{
		inner:   inner,
		ctr:     ctr,
		mac:     hmac.New(sha1.New, keys.auth),
		written: uint64(len(salt)) + 2,
	}, nil
}

func (w *aesWriter) Write(data []byte) (int, error) {
	if cap(w.buf) < len(data) {
		w.buf = make([]byte, len(data))
	}
	w.buf = w.buf[:len(data)]
	copy(w.buf, data)
	w.ctr.apply(w.buf)
	w.mac.Write(w.buf)
	if _, err := w.inner.Write(w.buf); err != nil {
		return 0, err
	}
	w.written += uint64(len(w.buf))
	return len(data), nil
}

// Finish appends the authentication code and reports the total number of bytes
// written through this writer.
func (w *aesWriter) Finish() (uint64, error) {
	tag := w.mac.Sum(nil)
	if _, err := w.inner.Write(tag[:10]); err != nil {
		return 0, fmt.Errorf("write error: %w", err)
	}
	w.written += 10
	return w.written, nil
}
