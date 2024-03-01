#[doc = "Register `MRCFG` reader"]
pub type R = crate::R<MRCFG_SPEC>;
#[doc = "Register `MRCFG` writer"]
pub type W = crate::W<MRCFG_SPEC>;
#[doc = "Message RAM Configuration\n\nYou can [`read`](crate::reg::generic::Reg::read) this register and get [`mrcfg::R`](R).  You can [`reset`](crate::reg::generic::Reg::reset), [`write`](crate::reg::generic::Reg::write), [`write_with_zero`](crate::reg::generic::Reg::write_with_zero) this register using [`mrcfg::W`](W). You can also [`modify`](crate::reg::generic::Reg::modify) this register. See [API](https://docs.rs/svd2rust/#read--modify--write-api)."]
pub struct MRCFG_SPEC;
impl crate::RegisterSpec for MRCFG_SPEC {
    type Ux = u32;
}
#[doc = "`read()` method returns [`mrcfg::R`](R) reader structure"]
impl crate::Readable for MRCFG_SPEC {}
#[doc = "`write(|w| ..)` method takes [`mrcfg::W`](W) writer structure"]
impl crate::Writable for MRCFG_SPEC {
    const ZERO_TO_MODIFY_FIELDS_BITMAP: Self::Ux = 0;
    const ONE_TO_MODIFY_FIELDS_BITMAP: Self::Ux = 0;
}
