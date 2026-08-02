/* STM32H750IBK6: 128K internal flash, ~1M SRAM split across four power domains. */

MEMORY
{
  FLASH  : ORIGIN = 0x08000000, LENGTH = 128K

  /* AXI SRAM (D1). DMA1/DMA2 reach this; they cannot reach DTCM or ITCM, which sit
     behind the CPU's TCM interface rather than on the bus matrix. */
  RAM    : ORIGIN = 0x24000000, LENGTH = 512K

  DTCM   : ORIGIN = 0x20000000, LENGTH = 128K
  ITCM   : ORIGIN = 0x00000000, LENGTH = 64K
  SRAM1  : ORIGIN = 0x30000000, LENGTH = 128K
  SRAM2  : ORIGIN = 0x30020000, LENGTH = 128K
  SRAM3  : ORIGIN = 0x30040000, LENGTH = 32K
  SRAM4  : ORIGIN = 0x38000000, LENGTH = 64K
}
