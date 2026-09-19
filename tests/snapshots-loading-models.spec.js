import { test, testPage } from "./lib.js";

test.describe("Model loading visual test", async () => {
    const testWithTimeout = (pageName) => testPage(pageName, async (page) => await page.waitForTimeout(3000));
    testWithTimeout("loading_gltf_duplex");
    testWithTimeout("loading_gltf_duck_quantized");
    testWithTimeout("loading_laz_autzen");
    testWithTimeout("loading_laz_1_4_copc");
    testWithTimeout("loading_laz_1_5");
    testWithTimeout("loading_las_1_5");
});
