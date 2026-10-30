#!/usr/bin/env python3
"""Generate the vdpu34x (rkvdec2) register map used by RK3588.

Source: mpp/hal/rkdec/inc/vdpu34x_com.h. The structs are laid out at fixed word
offsets:
    Vdpu34xRegComm        @   8
    Vdpu34xRegParam       @  64
    Vdpu34xRegCommAddr    @ 128
    Vdpu34xH2645HighPoc   @ 200
    Vdpu34xRegIrqStatus   @ 224
    Vdpu34xRegStatistic   @ 256
Each member is one word unless it is an array. Anonymous unions carry one
variant per codec; every variant is emitted under a prefix so H.264 fields are
reachable as `reg100.h264_*`.
"""
import re
import sys

BASES = [
    ("Vdpu34xRegComm", 8),
    ("Vdpu34xRegParam", 64),
    ("Vdpu34xRegCommAddr", 128),
    ("Vdpu34xH2645HighPoc", 200),
    ("Vdpu34xRegIrqStatus", 224),
    ("Vdpu34xRegStatistic", 256),
]

def strip_comments(t):
    t = re.sub(r'/\*.*?\*/', lambda m: '\x00' + m.group(0).replace('\n', ' ') + '\x00', t, flags=re.S)
    t = re.sub(r'//[^\n]*', '', t)
    return t

def comment_before(text, pos):
    # nearest \x00...\x00 comment before pos
    seg = text[:pos]
    i = seg.rfind('\x00')
    if i < 0:
        return None
    j = seg.rfind('\x00', 0, i)
    if j < 0:
        return None
    c = seg[j + 1:i].strip()
    return c

def split_members(body):
    """Yield (kind, text) at brace depth 1."""
    depth = 0
    cur = []
    i = 0
    while i < len(body):
        ch = body[i]
        if ch == '{':
            depth += 1
        elif ch == '}':
            depth -= 1
        cur.append(ch)
        if ch == ';' and depth == 0:
            yield ''.join(cur)
            cur = []
        i += 1

def parse_bits(s):
    """Fields from a plain struct body: list of (name|None, width, comment)."""
    out = []
    for m in re.finditer(r'(RK_U16|RK_U32)\s*(\w*)\s*(?::\s*(\d+))?\s*;', s):
        name = m.group(2) or None
        if m.group(3):
            width = int(m.group(3))
        else:
            width = 16 if m.group(1) == 'RK_U16' else 32
        out.append((name, width, comment_before(s, m.start())))
    return out

def parse_struct(name, text):
    body = text
    members = []
    for raw in split_members(body):
        txt = raw.strip().rstrip(';').strip()
        if not txt:
            continue
        arr = re.match(r'RK_U32\s+(\w+)\s*\[(\d+)\]', txt)
        if arr:
            members.append(('array', arr.group(1), int(arr.group(2)), []))
            continue
        # named member struct: "... } regNNN"
        m = re.match(r'(?:struct|union)\s*\{(.*)\}\s*(\w+)$', txt, re.S)
        if m:
            fields = parse_bits(m.group(1))
            members.append(('word', m.group(2), 1, fields))
            continue
        # anonymous struct/union (no trailing name)
        m = re.match(r'(?:struct|union)\s*\{(.*)\}$', txt, re.S)
        if m:
            inner = m.group(1)
            variants = []
            for sub in split_members(inner):
                sub = sub.strip().rstrip(';').strip()
                sm = re.match(r'struct\s*\{(.*)\}\s*(\w*)$', sub, re.S)
                if sm:
                    variants.append((sm.group(2) or '', parse_bits(sm.group(1))))
                elif sub.startswith('RK_U32'):
                    variants.append(('', parse_bits(sub)))
            # prefix per variant from its leading comment
            named = []
            for i, (nm, fields) in enumerate(variants):
                pre = None
                for f in fields:
                    if f[2]:
                        pre = f[2]
                        break
                prefix = (pre or nm or f'v{i}')
                prefix = re.sub(r'[^0-9A-Za-z]+', '_', prefix).strip('_').lower()
                named.append((prefix, fields))
            members.append(('union', '', 1, named))
            continue
        fields = parse_bits(txt)
        if fields:
            members.append(('word', '', 1, fields))
    return members

def main():
    path, out_path = sys.argv[1], sys.argv[2]
    text = strip_comments(open(path).read())
    out = []
    entries = []  # (word, name, lsb, width)
    for struct, base in BASES:
        m = re.search(
            r'typedef\s+struct\s+\w+\s*\{(.*?)\n\}\s*' + struct + r';', text, re.S
        )
        if not m:
            print("missing", struct, file=sys.stderr)
            continue
        members = parse_struct(struct, m.group(1))
        word = base
        lsb = 0
        for kind, name, count, fields in members:
            if kind == 'array':
                word += count + (1 if lsb else 0)
                lsb = 0
                continue
            if kind == 'union':
                if lsb:
                    word += 1
                    lsb = 0
                for prefix, fs in fields:
                    clsb = 0
                    cword = word
                    for fname, width, _c in fs:
                        if fname:
                            entries.append((cword, f'{prefix}_{fname}', clsb, width))
                        clsb += width
                        if clsb >= 32:
                            cword += 1
                            clsb = 0
                word += 1
                continue
            for fname, width, _c in fields:
                if lsb + width > 32:
                    word += 1
                    lsb = 0
                if fname:
                    entries.append((word, fname, lsb, width))
                lsb += width
                if lsb >= 32:
                    word += 1
                    lsb = 0

    n_words = 260
    out.append('//! vdpu34x (rkvdec2, RK3588) register map.')
    out.append('//!')
    out.append('//! Generated by `tools/gen_vdpu34x_regs.py` from the official MPP')
    out.append('//! `hal/rkdec/inc/vdpu34x_com.h`. Do not edit by hand.')
    out.append('')
    out.append(f'pub const N_WORDS: usize = {n_words};')
    out.append('')
    out.append('#[derive(Debug, Clone, Copy, PartialEq, Eq)]')
    out.append('pub struct Field {')
    out.append('    pub word: usize,')
    out.append('    pub lsb: u8,')
    out.append('    pub width: u8,')
    out.append('}')
    out.append('')
    out.append('#[derive(Clone)]')
    out.append('pub struct Regs {')
    out.append('    pub w: [u32; N_WORDS],')
    out.append('}')
    out.append('')
    out.append('impl Default for Regs {')
    out.append('    fn default() -> Self { Regs { w: [0; N_WORDS] } }')
    out.append('}')
    out.append('')
    out.append('impl Regs {')
    out.append('    pub fn new() -> Self { Self::default() }')
    out.append('')
    out.append('    #[inline]')
    out.append('    pub fn set(&mut self, f: Field, v: u32) -> &mut Self {')
    out.append('        let mask = if f.width >= 32 { u32::MAX } else { (1u32 << f.width) - 1 };')
    out.append('        let s = f.lsb as u32;')
    out.append('        self.w[f.word] = (self.w[f.word] & !(mask << s)) | ((v & mask) << s);')
    out.append('        self')
    out.append('    }')
    out.append('')
    out.append('    #[inline]')
    out.append('    pub fn get(&self, f: Field) -> u32 {')
    out.append('        let mask = if f.width >= 32 { u32::MAX } else { (1u32 << f.width) - 1 };')
    out.append('        (self.w[f.word] >> f.lsb) & mask')
    out.append('    }')
    out.append('}')
    out.append('')

    by_word = {}
    for word, name, lsb, width in entries:
        by_word.setdefault(word, []).append((name, lsb, width))
    for word in sorted(by_word):
        out.append(f'/// word {word}')
        out.append(f'pub mod w{word} {{')
        out.append('    use super::Field;')
        seen = {}
        for name, lsb, width in by_word[word]:
            key = name.upper()
            if key in seen:
                if lsb == seen[key]:
                    continue
                seen[key] += 1
                key = f'{key}_{seen[key]}'
            else:
                seen[key] = 0
            out.append(f'    pub const {key}: Field = Field {{ word: {word}, lsb: {lsb}, width: {width} }};')
        out.append('}')
        out.append('')

    open(out_path, 'w').write('\n'.join(out) + '\n')
    print(f"{len(entries)} fields, {len(by_word)} words -> {out_path}")

if __name__ == '__main__':
    main()
