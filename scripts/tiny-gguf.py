# Write a tiny llama GGUF with random Q8_0 weights and a byte vocabulary:
#
#   python3 scripts/tiny-gguf.py <file.gguf>     needs the gguf and numpy packages
#
# A load and a completion of it run the CPU kernels of llama.cpp, like a real
# cleanup model, in a moment. The text it writes is noise.
# scripts/test-cleanup-cpus.sh uses it.
import sys
import numpy as np
import gguf
from gguf import GGMLQuantizationType as Q
from gguf import quants

out = sys.argv[1]
rng = np.random.default_rng(1)
n_vocab, n_embd, n_ff, n_head, n_layer = 288, 256, 512, 4, 2

w = gguf.GGUFWriter(out, "llama")
w.add_name("tiny")
w.add_context_length(512)
w.add_embedding_length(n_embd)
w.add_feed_forward_length(n_ff)
w.add_block_count(n_layer)
w.add_head_count(n_head)
w.add_head_count_kv(n_head)
w.add_rope_dimension_count(n_embd // n_head)
w.add_layer_norm_rms_eps(1e-5)
w.add_file_type(gguf.LlamaFileType.MOSTLY_Q8_0)

tokens, scores, types = ["<unk>", "<s>", "</s>"], [0.0, 0.0, 0.0], [2, 3, 3]
for b in range(256):
    tokens.append(f"<0x{b:02X}>"); scores.append(0.0); types.append(6)
for i, t in enumerate(["▁", "▁the", "▁a", "e", "t", "o", "n", "s", "r", "h", "l", "d", "c", "u", "m", "f", "p", "g", "w", "y", "b", "v", "k", "x", "j", "q", "z", "i", "a"][: n_vocab - len(tokens)]):
    tokens.append(t); scores.append(-float(i)); types.append(1)
w.add_tokenizer_model("llama")
w.add_token_list(tokens)
w.add_token_scores(scores)
w.add_token_types(types)
w.add_bos_token_id(1)
w.add_eos_token_id(2)
w.add_unk_token_id(0)

def q8(name, shape):
    data = (rng.standard_normal(shape) * 0.02).astype(np.float32)
    w.add_tensor(name, quants.quantize(data, Q.Q8_0), raw_dtype=Q.Q8_0)

def f32(name, shape):
    w.add_tensor(name, np.ones(shape, dtype=np.float32))

q8("token_embd.weight", (n_vocab, n_embd))
for i in range(n_layer):
    f32(f"blk.{i}.attn_norm.weight", (n_embd,))
    for t in ("attn_q", "attn_k", "attn_v", "attn_output"):
        q8(f"blk.{i}.{t}.weight", (n_embd, n_embd))
    f32(f"blk.{i}.ffn_norm.weight", (n_embd,))
    q8(f"blk.{i}.ffn_gate.weight", (n_ff, n_embd))
    q8(f"blk.{i}.ffn_up.weight", (n_ff, n_embd))
    q8(f"blk.{i}.ffn_down.weight", (n_embd, n_ff))
f32("output_norm.weight", (n_embd,))
q8("output.weight", (n_vocab, n_embd))

w.write_header_to_file()
w.write_kv_data_to_file()
w.write_tensors_to_file()
w.close()
