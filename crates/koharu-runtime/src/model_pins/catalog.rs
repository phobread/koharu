// Audited 2026-09-14. See docs/en-US/how-to/verify-model-pins.md.
// Cached baselines were hash-verified; uncached entries pin metadata only.
use super::{ModelFile, ModelPin};

pub(super) const PINS: &[ModelPin] = &[
    ModelPin {
        repo: "black-forest-labs/FLUX.2-small-decoder",
        revision: "a3efc24f613ef42d9428af62fdbd6f5fd8856c4a",
        files: &[
            ModelFile {
                filename: "config.json",
                oid: "8c88d9f9c8a262fd323b3d0706ee3e912ba17296",
                size: 842,
            },
            ModelFile {
                filename: "diffusion_pytorch_model.safetensors",
                oid: "d8d52ba036475f5fb07c8b435e176d3d97ebfa82f0d1a1c317f9cc1e25bd013b",
                size: 249521340,
            },
        ],
    },
    ModelPin {
        repo: "fffonion/yuzumarker-font-detection",
        revision: "7484242bb840f39e27f10df8ade1f8a9a8fa8f53",
        files: &[
            ModelFile {
                filename: "font-labels-ex.json",
                oid: "31d8f806caede0366606ac05662737cd0aeb3fb7",
                size: 715294,
            },
            ModelFile {
                filename: "yuzumarker-font-detection.safetensors",
                oid: "eb82b26e03be28d1e547d86d7668fb7c712c2c42e190cf89b008e25dd507a9d7",
                size: 144784832,
            },
        ],
    },
    ModelPin {
        repo: "mayocream/anime-text-yolo",
        revision: "937f67dfe61fc4793549782e103751fdc1f0a8d9",
        files: &[
            ModelFile {
                filename: "config.json",
                oid: "7823b652ed505bbec4ae87abf39f5b5d94df8c26",
                size: 389,
            },
            ModelFile {
                filename: "yolo12l_animetext.safetensors",
                oid: "24ef617a079a86259c750ba25de1d788c0bcc75440d09b9427cf309b56da580a",
                size: 106088076,
            },
            ModelFile {
                filename: "yolo12m_animetext.safetensors",
                oid: "cf698e608d772b0d5997adcee5cc9c372979408eb85968f0c2d3fac0fd3de593",
                size: 80891492,
            },
            ModelFile {
                filename: "yolo12n_animetext.safetensors",
                oid: "79bbe16deb26aff8094ebf1f262f74062a4a25df0c47ab0ddbcf20305c7b68eb",
                size: 10433860,
            },
            ModelFile {
                filename: "yolo12s_animetext.safetensors",
                oid: "02688ce956c23c654596be1e02764f016ac00b2559b16091ca5b66551358e2ff",
                size: 37263660,
            },
            ModelFile {
                filename: "yolo12x_animetext.safetensors",
                oid: "b82ff82fb216a621cc0857baf33cf1c9fa14baea76becad2bebc95906474b9fb",
                size: 237208044,
            },
        ],
    },
    ModelPin {
        repo: "mayocream/aot-inpainting",
        revision: "bde6131f9d3ef841b435507def8534715ac8e87c",
        files: &[
            ModelFile {
                filename: "config.json",
                oid: "44379dd62b55e4857d38f596bf911427bc40d2e4",
                size: 481,
            },
            ModelFile {
                filename: "model.safetensors",
                oid: "1b4fea17a84a228c2097a42ab2f403357f07bb56ae022dc243b40817b7aa87d1",
                size: 22732864,
            },
        ],
    },
    ModelPin {
        repo: "mayocream/comic-text-detector",
        revision: "15ade029f4dabd502bc97af6051c8b9f2bec24d5",
        files: &[
            ModelFile {
                filename: "config.json",
                oid: "4d71d92d4c8ef4618529aa93ffc3576aff37447a",
                size: 412,
            },
            ModelFile {
                filename: "dbnet.safetensors",
                oid: "6122c6fb14a07701826f8bc2d548ffcc3b4da42fcc25a26e2dadfb9f1bca427a",
                size: 16686024,
            },
            ModelFile {
                filename: "unet.safetensors",
                oid: "6c3d7525a77847494c5a383d97d85ee24871ab61602a74d1a802dc58df770baa",
                size: 48981624,
            },
            ModelFile {
                filename: "yolo-v5.safetensors",
                oid: "95909fdde609beb5cc2c69fc63dd146b886507312ceda83c5147f30330c71bf1",
                size: 14120386,
            },
        ],
    },
    ModelPin {
        repo: "mayocream/lama-manga",
        revision: "f91c85b26913b3e83f9877867b4c336da3675238",
        files: &[
            ModelFile {
                filename: "config.json",
                oid: "7ccd4ea885dfd37d7907544723bf4506393a0993",
                size: 379,
            },
            ModelFile {
                filename: "lama-manga.safetensors",
                oid: "a790515e9da839b8d89af7d565ceb110d908b7d6fbdb991f2acb2ec7d9b08bdb",
                size: 204332996,
            },
        ],
    },
    ModelPin {
        repo: "mayocream/manga-ocr",
        revision: "4380edba990b959c508752350955350c1c80c31c",
        files: &[
            ModelFile {
                filename: "config.json",
                oid: "deb78906462c7ea5a5b891c7873461c5fed28d0d",
                size: 77546,
            },
            ModelFile {
                filename: "model.safetensors",
                oid: "75cbff89aa1c03330b966b24867fd656a953a7df7e8f938a7a061d2b0eb4eb67",
                size: 462959024,
            },
            ModelFile {
                filename: "preprocessor_config.json",
                oid: "b7414e73cf93e2818ed2c82d3d7bfc0d85991c13",
                size: 228,
            },
            ModelFile {
                filename: "special_tokens_map.json",
                oid: "e7b0375001f109a6b8873d756ad4f7bbb15fbaa5",
                size: 112,
            },
            ModelFile {
                filename: "vocab.txt",
                oid: "c2bd4e463e91d61f0c6e9a0cdaa221a50384c4dc",
                size: 24072,
            },
        ],
    },
    ModelPin {
        repo: "mayocream/manga-text-segmentation-2025",
        revision: "efd866e3ac6595ea20722f35ae343c403056ba76",
        files: &[ModelFile {
            filename: "model.safetensors",
            oid: "f7e68a0c3e53dda4aba056a2370bcfb99f4dd189e9867d4570bc12578f9c7c4a",
            size: 216516884,
        }],
    },
    ModelPin {
        repo: "mayocream/mit48px-ocr",
        revision: "205395b155a041b068fd754a6e417cd71b4cb1de",
        files: &[
            ModelFile {
                filename: "alphabet-all-v7.txt",
                oid: "af4466e3a4e4043f8c7990e679a742d6354c1a9a",
                size: 186651,
            },
            ModelFile {
                filename: "config.json",
                oid: "7c251b73496541ab6586cc3a514f12587615b7d4",
                size: 334,
            },
            ModelFile {
                filename: "model.safetensors",
                oid: "462fe265c21e15c95fec6920c408b84c3e21bae03cce2eb5686f0aef155befdd",
                size: 263375432,
            },
        ],
    },
    ModelPin {
        repo: "mayocream/speech-bubble-segmentation",
        revision: "387bc1e93f3d24702bc8609798b6a13b37420edc",
        files: &[
            ModelFile {
                filename: "config.json",
                oid: "1d25b7ec6495a0c2bd976371419bc7135a24c6ff",
                size: 433,
            },
            ModelFile {
                filename: "model.safetensors",
                oid: "c881d96771755fa628a94bb5f4b18301a0728ae4ffe8f14b2e9dde55e1b40552",
                size: 109141868,
            },
        ],
    },
    ModelPin {
        repo: "ogkalu/comic-text-and-bubble-detector",
        revision: "16e8a622f91fabc6b5b65c96d32d1183f8843546",
        files: &[
            ModelFile {
                filename: "config.json",
                oid: "59bc8ad3e2d84dbbdf9258accf50c73131b467d8",
                size: 2320,
            },
            ModelFile {
                filename: "model.safetensors",
                oid: "037930a861e67870eb345be01b28cc70d7e2b7956528e48ee0ebdb0c093df80d",
                size: 171543900,
            },
            ModelFile {
                filename: "preprocessor_config.json",
                oid: "ea1fbb1edd242104e70301472d089735c68c5f21",
                size: 470,
            },
        ],
    },
    ModelPin {
        repo: "PaddlePaddle/PaddleOCR-VL-1.6",
        revision: "c5630abae1d940eafe0697512a0325494b02ab42",
        files: &[
            ModelFile {
                filename: "config.json",
                oid: "c54711466ae75457f8e57b909a49dae570bfa5c5",
                size: 2059,
            },
            ModelFile {
                filename: "model.safetensors",
                oid: "85a479d506a11e724e7285d395c551be69f41dbc16b6342d3cacfb189aed71db",
                size: 1917255968,
            },
            ModelFile {
                filename: "preprocessor_config.json",
                oid: "d873cd63d04d9e6243999759fc30f7752dab3222",
                size: 641,
            },
            ModelFile {
                filename: "special_tokens_map.json",
                oid: "dcd70aaa8c9987899d7593995546d9c0cfc6a1f3",
                size: 1151,
            },
            ModelFile {
                filename: "tokenizer.json",
                oid: "c8a215a59183d0d0781adc33bacd3ce6162716f7fd568fb30234a74d69803a7d",
                size: 11189060,
            },
        ],
    },
    ModelPin {
        repo: "PaddlePaddle/PaddleOCR-VL-1.6-GGUF",
        revision: "511b09642bb324401f15f97cc23bc67e8f0a291d",
        files: &[
            ModelFile {
                filename: "PaddleOCR-VL-1.6-GGUF-mmproj.gguf",
                oid: "204d757d7610d9b3faab10d506d69e5b244e32bf765e2bab2d0167e65e0a058a",
                size: 881770560,
            },
            ModelFile {
                filename: "PaddleOCR-VL-1.6-GGUF.gguf",
                oid: "f3ae46ec885050acf4b3d31944431e1fd90d50664fb09126af4a3c050ba14ee8",
                size: 935769056,
            },
        ],
    },
    ModelPin {
        repo: "PaddlePaddle/PP-DocLayoutV3_safetensors",
        revision: "3ec586e86ed9245a567bb13395a3db64d5c077cc",
        files: &[
            ModelFile {
                filename: "config.json",
                oid: "5a22928c191950850cbc0e56e43f722073e7c8da",
                size: 2460,
            },
            ModelFile {
                filename: "model.safetensors",
                oid: "5ea422c6cc5fe759a47e1357c35639b58173508e025a3131cbe4b6ac59e2b85e",
                size: 133270468,
            },
            ModelFile {
                filename: "preprocessor_config.json",
                oid: "ab66797648e5a3247eca2988e9fcd8af07a6a038",
                size: 575,
            },
        ],
    },
    ModelPin {
        repo: "Qwen/Qwen3-4B",
        revision: "1cfa9a7208912126459214e8b04321603b3df60c",
        files: &[
            ModelFile {
                filename: "config.json",
                oid: "e49eccdc32f36da9c09cfa0e737084f9e0105e5e",
                size: 726,
            },
            ModelFile {
                filename: "tokenizer.json",
                oid: "aeb13307a71acd8fe81861d94ad54ab689df773318809eed3cbe794b4492dae4",
                size: 11422654,
            },
        ],
    },
    ModelPin {
        repo: "unsloth/FLUX.2-klein-4B-GGUF",
        revision: "0084d1df98e2e2137fe776d55170bc4792ec1d66",
        files: &[ModelFile {
            filename: "flux-2-klein-4b-Q4_K_M.gguf",
            oid: "0b25d143c8469b342bc5af3bce92b783bf6b0636d285f7b2f75e38af63af9a15",
            size: 2604311104,
        }],
    },
    ModelPin {
        repo: "unsloth/Qwen3-4B-GGUF",
        revision: "22c9fc8a8c7700b76a1789366280a6a5a1ad1120",
        files: &[
            ModelFile {
                filename: "Qwen3-4B-Q4_K_M.gguf",
                oid: "f6f851777709861056efcdad3af01da38b31223a3ba26e61a4f8bf3a2195813a",
                size: 2497281312,
            },
            ModelFile {
                filename: "config.json",
                oid: "fb75e27293471d40ec9444454638fafa1ccdbe56",
                size: 752,
            },
        ],
    },
];
