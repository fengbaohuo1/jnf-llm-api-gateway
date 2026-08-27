import sharp from "sharp";
const names = ["gateway-fan", "layers", "network"];
for (const n of names) {
  await sharp(`brand/${n}.svg`).resize(1024, 1024).png().toFile(`brand/${n}-1024.png`);
  await sharp(`brand/${n}.svg`).resize(512, 512).png().toFile(`brand/${n}-preview.png`);
  console.log("rendered", n);
}
