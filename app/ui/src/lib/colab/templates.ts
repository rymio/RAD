// Starter notebooks for Rust Colab and Python Colab: short, runnable lessons in the maths behind
// machine learning and LLMs. Each says which pack it needs, so it also works offline once that
// pack is downloaded.

import type { Lang } from './notebook';

export interface Template {
  id: string;
  lang: Lang;
  title: string;
  about: string;
  /** The pack it uses (Packages & offline), if any. */
  pack?: string;
  cells: { md?: string; code?: string }[];
}

const py = (id: string, title: string, about: string, pack: string | undefined, cells: Template['cells']): Template => ({ id, lang: 'python', title, about, pack, cells });
const rs = (id: string, title: string, about: string, pack: string | undefined, cells: Template['cells']): Template => ({ id, lang: 'rust', title, about, pack, cells });

export const TEMPLATES: Template[] = [
  // ---------------------------------------------------------------- Python
  py('py-blank', 'Blank notebook', 'An empty Python notebook.', undefined, [{ code: '' }]),
  py('py-welcome', 'Welcome to Python Colab', 'How the notebook works: cells, outputs, packages, offline.', undefined, [
    {
      md: `# Welcome to Python Colab

Each **code cell** runs in the same Python, so what one cell defines the next can use.

- **Shift + Enter** runs a cell and moves on; **⌘ + Enter** runs it and stays.
- The last line's value is shown, like in Jupyter (end the line with \`;\` to hide it). \`display(x)\` shows anything along the way.
- \`%pip install name\` adds a package. Online, it's saved in Colab's local packages first, so it's there when you're offline.
- \`!command\` runs a shell command in the notebook's folder.
- **Markdown cells** take maths: $e^{i\\pi} + 1 = 0$.

Ask **Gilgamesh** (on the right) about anything: what a cell does, why it fails, the maths behind it. He sees the notebook and its outputs, and searches your vault (the PDFs and notes you've added to RAG).`,
    },
    { code: 'x = 21\nx * 2' },
    { code: 'for i in range(3):\n    print("line", i)\nsum(range(101))' },
    { code: 'import sys, platform\nprint(sys.version)\nplatform.machine()' },
  ]),
  py('py-linalg', 'Linear algebra with NumPy', 'Vectors, matrices, eigenvalues and a picture of a linear map.', 'maths', [
    {
      md: `# Linear algebra with NumPy

A matrix $A$ is a linear map: it sends a vector $\\mathbf{x}$ to $A\\mathbf{x}$. Its **eigenvectors** are the directions it only stretches: $A\\mathbf{v} = \\lambda\\mathbf{v}$.`,
    },
    { code: 'import numpy as np\n\nA = np.array([[2.0, 1.0],\n              [1.0, 3.0]])\nx = np.array([1.0, 0.0])\nA @ x' },
    { code: '# eigh: for symmetric matrices (real eigenvalues, in increasing order)\nvalues, vectors = np.linalg.eigh(A)\nprint("eigenvalues:", values)\nprint("eigenvectors (columns):\\n", vectors)\n# Check A v = λ v for the first one\nv = vectors[:, 0]\nnp.allclose(A @ v, values[0] * v)' },
    { md: 'The unit circle under $A$ becomes an ellipse; its axes are the eigenvectors, scaled by the eigenvalues (here $A$ is symmetric, so they are at right angles).' },
    {
      code: `import matplotlib.pyplot as plt

t = np.linspace(0, 2 * np.pi, 200)
circle = np.stack([np.cos(t), np.sin(t)])
ellipse = A @ circle

plt.figure(figsize=(5, 5))
plt.plot(*circle, label="unit circle")
plt.plot(*ellipse, label="A · circle")
for lam, v in zip(values, vectors.T):
    plt.arrow(0, 0, *(lam * v), width=0.03, color="crimson", length_includes_head=True)
plt.axis("equal"); plt.grid(alpha=0.3); plt.legend()
plt.title("A linear map and its eigenvectors");`,
    },
    { code: 'print("determinant:", np.linalg.det(A), "= product of eigenvalues:", values.prod())\nprint("inverse:\\n", np.linalg.inv(A))\nnp.linalg.solve(A, np.array([3.0, 5.0]))  # x with A x = b' },
  ]),
  py('py-gd', 'Gradient descent from scratch', 'Fit a line by following the gradient of the loss; watch it learn.', 'maths', [
    {
      md: `# Gradient descent from scratch

We fit $\\hat y = w x + b$ to noisy points by minimising the mean squared error
$$L(w, b) = \\frac{1}{n}\\sum_i (w x_i + b - y_i)^2 .$$
Its gradient is $\\frac{\\partial L}{\\partial w} = \\frac{2}{n}\\sum_i (\\hat y_i - y_i)\\,x_i$ and $\\frac{\\partial L}{\\partial b} = \\frac{2}{n}\\sum_i (\\hat y_i - y_i)$. Each step moves $w$ and $b$ a little against it: $w \\leftarrow w - \\eta\\,\\partial L/\\partial w$.`,
    },
    { code: 'import numpy as np\nimport matplotlib.pyplot as plt\n\nrng = np.random.default_rng(0)\nx = rng.uniform(-3, 3, 100)\ny = 1.7 * x - 0.5 + rng.normal(0, 0.8, 100)\nplt.scatter(x, y, s=12); plt.title("data: y ≈ 1.7x − 0.5 + noise");' },
    {
      code: `w, b, lr = 0.0, 0.0, 0.05
history = []
for step in range(200):
    pred = w * x + b
    err = pred - y
    loss = np.mean(err ** 2)
    w -= lr * 2 * np.mean(err * x)
    b -= lr * 2 * np.mean(err)
    history.append(loss)
print(f"w = {w:.3f}, b = {b:.3f}, loss = {loss:.3f}")`,
    },
    { code: 'fig, ax = plt.subplots(1, 2, figsize=(10, 4))\nax[0].plot(history); ax[0].set_yscale("log"); ax[0].set_title("loss per step")\nax[1].scatter(x, y, s=12); xs = np.linspace(-3, 3, 2)\nax[1].plot(xs, w * xs + b, color="crimson"); ax[1].set_title("the fitted line");' },
    { md: 'Try a learning rate of `0.5` instead of `0.05`: the loss grows instead of shrinking. Ask Gilgamesh why.' },
  ]),
  py('py-nn', 'A tiny neural network (PyTorch)', 'Train a two-layer network to separate two spirals.', 'ml', [
    {
      md: `# A tiny neural network in PyTorch

Two interleaved spirals can't be split by a straight line, but a network with a hidden layer can learn a curved boundary.
A layer computes $h = \\tanh(W x + b)$; training adjusts every $W$ and $b$ by gradient descent, with the gradients worked out for us by **autograd**.`,
    },
    {
      code: `import numpy as np, torch, torch.nn as nn
import matplotlib.pyplot as plt
torch.manual_seed(0)

n = 300
t = torch.linspace(0.5, 3.5 * np.pi, n)
a = torch.stack([t * torch.cos(t), t * torch.sin(t)], 1) / 10
X = torch.cat([a, -a]) + 0.03 * torch.randn(2 * n, 2)
y = torch.cat([torch.zeros(n), torch.ones(n)])
plt.scatter(X[:, 0], X[:, 1], c=y, cmap="coolwarm", s=8); plt.axis("equal");`,
    },
    {
      code: `model = nn.Sequential(nn.Linear(2, 64), nn.Tanh(), nn.Linear(64, 64), nn.Tanh(), nn.Linear(64, 1))
opt = torch.optim.Adam(model.parameters(), lr=0.01)
loss_fn = nn.BCEWithLogitsLoss()

for epoch in range(1500):
    opt.zero_grad()
    loss = loss_fn(model(X).squeeze(1), y)
    loss.backward()
    opt.step()
    if epoch % 300 == 0:
        print(f"epoch {epoch:4d}  loss {loss.item():.4f}")
accuracy = ((model(X).squeeze(1) > 0).float() == y).float().mean()
f"accuracy {accuracy:.1%}"`,
    },
    {
      code: `xx, yy = torch.meshgrid(torch.linspace(-1.2, 1.2, 200), torch.linspace(-1.2, 1.2, 200), indexing="xy")
with torch.no_grad():
    zz = torch.sigmoid(model(torch.stack([xx.ravel(), yy.ravel()], 1))).reshape(xx.shape)
plt.contourf(xx, yy, zz, levels=20, cmap="coolwarm", alpha=0.6)
plt.scatter(X[:, 0], X[:, 1], c=y, cmap="coolwarm", s=6, edgecolors="none"); plt.axis("equal")
plt.title("what the network learned");`,
    },
  ]),
  py('py-attention', 'Attention, step by step', 'The core of a transformer (LLM) in a few lines of NumPy.', 'maths', [
    {
      md: `# Attention, step by step

In a transformer each token looks at the others and takes a weighted mix of what they carry. With queries $Q$, keys $K$ and values $V$:
$$\\operatorname{Attention}(Q, K, V) = \\operatorname{softmax}\\!\\left(\\frac{Q K^\\top}{\\sqrt{d_k}}\\right) V$$
The softmax turns scores into weights that add up to 1: $\\operatorname{softmax}(z)_i = e^{z_i} / \\sum_j e^{z_j}$.`,
    },
    { code: 'import numpy as np\nnp.set_printoptions(precision=3, suppress=True)\n\ndef softmax(z, axis=-1):\n    z = z - z.max(axis=axis, keepdims=True)   # same result, no overflow\n    e = np.exp(z)\n    return e / e.sum(axis=axis, keepdims=True)\n\nsoftmax(np.array([2.0, 1.0, 0.1]))' },
    {
      code: `rng = np.random.default_rng(1)
tokens = ["the", "cat", "sat", "down"]
d_model, d_k = 8, 4
E = rng.normal(size=(len(tokens), d_model))          # one embedding per token
Wq, Wk, Wv = (rng.normal(size=(d_model, d_k)) for _ in range(3))
Q, K, V = E @ Wq, E @ Wk, E @ Wv

scores = Q @ K.T / np.sqrt(d_k)
# A causal mask: a token only sees the ones before it (as in GPT)
scores = np.where(np.tril(np.ones_like(scores)) == 1, scores, -np.inf)
weights = softmax(scores)
weights`,
    },
    { code: 'import matplotlib.pyplot as plt\nplt.imshow(weights, cmap="viridis")\nplt.xticks(range(4), tokens); plt.yticks(range(4), tokens)\nplt.xlabel("looks at"); plt.ylabel("token"); plt.colorbar(); plt.title("attention weights");' },
    { code: 'out = weights @ V   # each row: the mix of values this token takes\nout.shape, out' },
    { md: 'With the **LLMs** pack, try `from transformers import AutoTokenizer` to see how real text becomes tokens (the model files are downloaded once, then kept).' },
  ]),

  // ---------------------------------------------------------------- Rust
  rs('rs-blank', 'Blank notebook', 'An empty Rust notebook.', undefined, [{ code: '' }]),
  rs('rs-welcome', 'Welcome to Rust Colab', 'How the notebook works: cells, values, crates, offline.', undefined, [
    {
      md: `# Welcome to Rust Colab

Rust, a cell at a time (with [evcxr](https://github.com/evcxr/evcxr)). Variables, functions and types defined in one cell are there in the next.

- **Shift + Enter** runs a cell and moves on; **⌘ + Enter** runs it and stays.
- A cell's last expression, without a \`;\`, is shown (with \`Debug\`).
- \`:dep ndarray = "0.17"\` adds a crate. Crates are downloaded once into Colab's own store and built from there, so they work offline too (**Packages & offline** downloads the important ones ahead).
- The first cell, and the first use of a crate, take a while to build; after that it's quick.
- \`:vars\` lists the variables; \`:help\` lists evcxr's other commands.

Ask **Gilgamesh** (on the right) about anything: ownership, the borrow checker, what a compiler error means, the maths. He sees the notebook and its errors, and searches your vault.`,
    },
    { code: 'let x = 21;\nx * 2' },
    { code: 'fn factorial(n: u64) -> u64 {\n    (1..=n).product()\n}\nfactorial(20)' },
    { code: 'let words = vec!["ferris", "the", "crab"];\nlet lengths: Vec<usize> = words.iter().map(|w| w.len()).collect();\nprintln!("{words:?} → {lengths:?}");\nlengths.iter().sum::<usize>()' },
  ]),
  rs('rs-gd', 'Gradient descent in plain Rust', 'Fit a line with no crates at all: the loop behind machine learning.', undefined, [
    {
      md: `# Gradient descent in plain Rust

We fit $\\hat y = w x + b$ by minimising $L = \\frac{1}{n}\\sum_i (\\hat y_i - y_i)^2$, stepping $w$ and $b$ against the gradient:
$\\frac{\\partial L}{\\partial w} = \\frac{2}{n}\\sum_i(\\hat y_i - y_i)\\,x_i$, $\\frac{\\partial L}{\\partial b} = \\frac{2}{n}\\sum_i(\\hat y_i - y_i)$.`,
    },
    {
      code: `// A small pseudo-random generator (xorshift), so this needs no crates.
fn next(seed: &mut u64) -> f64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    (*seed % 10_000) as f64 / 10_000.0
}
let mut seed: u64 = 42;
let xs: Vec<f64> = (0..100).map(|_| next(&mut seed) * 6.0 - 3.0).collect();
let ys: Vec<f64> = xs.iter().map(|x| 1.7 * x - 0.5 + (next(&mut seed) - 0.5)).collect();
(xs.len(), xs[..3].to_vec())`,
    },
    {
      code: `let (mut w, mut b, lr) = (0.0f64, 0.0f64, 0.05);
let n = xs.len() as f64;
for step in 0..200 {
    let (mut gw, mut gb, mut loss) = (0.0, 0.0, 0.0);
    for (x, y) in xs.iter().zip(&ys) {
        let err = w * x + b - y;
        gw += 2.0 * err * x / n;
        gb += 2.0 * err / n;
        loss += err * err / n;
    }
    w -= lr * gw;
    b -= lr * gb;
    if step % 50 == 0 { println!("step {step:3}  loss {loss:.4}"); }
}
format!("w = {w:.3}, b = {b:.3}")`,
    },
  ]),
  rs('rs-ndarray', 'Matrices with ndarray', 'N-dimensional arrays, matrix products and broadcasting.', 'maths', [
    { md: '# Matrices with ndarray\n\n`ndarray` is Rust\'s NumPy: arrays of any shape, slicing, and fast arithmetic. Building it the first time takes a few seconds.' },
    { code: ':dep ndarray = "0.17"\nuse ndarray::{array, Array, Array2, Axis};\nlet a = array![[2.0, 1.0], [1.0, 3.0]];\nlet x = array![1.0, 0.0];\na.dot(&x)' },
    { code: 'let i: Array2<f64> = Array2::eye(2);\nlet m = &a * 2.0 + &i;      // elementwise, by reference: a stays usable\nm.dot(&a)' },
    { code: 'let grid = Array::linspace(0.0, 1.0, 6).into_shape_with_order((2, 3)).unwrap();\nprintln!("{grid}");\nprintln!("sum of each column: {}", grid.sum_axis(Axis(0)));\ngrid.mapv(|v: f64| v.powi(2))' },
  ]),
  rs('rs-plot', 'Charts with plotters', 'Draw a function and a histogram right in the notebook.', 'tools', [
    { md: '# Charts with plotters\n\n`evcxr_figure` draws into an SVG that the notebook shows. The first build of `plotters` takes a little while.' },
    {
      code: `:dep plotters = { version = "0.3", features = ["evcxr"] }
use plotters::prelude::*;
evcxr_figure((560, 320), |root| {
    root.fill(&WHITE)?;
    let mut chart = ChartBuilder::on(&root)
        .caption("sin x and its Taylor polynomial", ("sans-serif", 18))
        .margin(10).x_label_area_size(30).y_label_area_size(40)
        .build_cartesian_2d(-4f64..4f64, -2f64..2f64)?;
    chart.configure_mesh().draw()?;
    let xs = (-400..=400).map(|i| i as f64 / 100.0);
    chart.draw_series(LineSeries::new(xs.clone().map(|x| (x, x.sin())), &BLUE))?
        .label("sin x").legend(|(x, y)| PathElement::new([(x, y), (x + 20, y)], BLUE));
    chart.draw_series(LineSeries::new(xs.map(|x| (x, x - x.powi(3) / 6.0 + x.powi(5) / 120.0)), &RED))?
        .label("x − x³/3! + x⁵/5!").legend(|(x, y)| PathElement::new([(x, y), (x + 20, y)], RED));
    chart.configure_series_labels().background_style(WHITE.mix(0.8)).border_style(BLACK).draw()?;
    Ok(())
})`,
    },
  ]),
  rs('rs-candle', 'Tensors with candle', 'Hugging Face\'s Rust ML library: tensors, matmul, softmax.', 'ml', [
    { md: '# Tensors with candle\n\n[candle](https://github.com/huggingface/candle) runs models (LLMs too) in pure Rust. Its first build takes about half a minute.' },
    { code: ':dep candle-core = "0.11"\nuse candle_core::{Device, Tensor, D};\nlet dev = Device::Cpu;\nlet a = Tensor::new(&[[1f32, 2.], [3., 4.]], &dev)?;\nprintln!("{}", a.matmul(&a.t()?)?);' },
    {
      code: `// softmax(z)_i = exp(z_i) / Σ_j exp(z_j), along the last dimension
let z = Tensor::new(&[[2f32, 1.0, 0.1]], &dev)?;
let e = z.broadcast_sub(&z.max_keepdim(D::Minus1)?)?.exp()?;
let p = e.broadcast_div(&e.sum_keepdim(D::Minus1)?)?;
println!("{p}");`,
    },
  ]),
];

export function templatesFor(lang: Lang) {
  return TEMPLATES.filter((t) => t.lang === lang);
}
