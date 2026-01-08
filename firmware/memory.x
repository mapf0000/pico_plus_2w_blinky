MEMORY {
    /*
     * The RP2350 has either external or internal flash.
     * Pimoroni Pico Plus 2 W ships with 16 MiB external flash.
     * (Board also includes 8 MiB PSRAM that firmware uses when the
     * `psram` feature is enabled.)
     */
    /*
     * Reserve 8 MiB for the read-only USB MSC image and 8 KiB at the end of
     * flash for persistent config.
     */
    FLASH : ORIGIN = 0x10000000, LENGTH = 8184K
    MSC : ORIGIN = 0x10000000 + 8184K, LENGTH = 8192K
    PERSIST : ORIGIN = 0x10000000 + 8184K + 8192K, LENGTH = 8K
    /*
     * RAM consists of 8 banks, SRAM0-SRAM7, with a striped mapping.
     * This is usually good for performance, as it distributes load on
     * those banks evenly.
     */
    RAM : ORIGIN = 0x20000000, LENGTH = 512K
    /*
     * RAM banks 8 and 9 use a direct mapping. They can be used to have
     * memory areas dedicated for some specific job, improving predictability
     * of access times.
     * Example: Separate stacks for core0 and core1.
     */
    SRAM4 : ORIGIN = 0x20080000, LENGTH = 4K
    SRAM5 : ORIGIN = 0x20081000, LENGTH = 4K
}

SECTIONS {
    /* ### Boot ROM info
     *
     * Goes after .vector_table, to keep it in the first 4K of flash
     * where the Boot ROM (and picotool) can find it
     */
    .start_block : ALIGN(4)
    {
        __start_block_addr = .;
        KEEP(*(.start_block));
        KEEP(*(.boot_info));
    } > FLASH

} INSERT AFTER .vector_table;

/* move .text to start /after/ the boot info */
_stext = ADDR(.start_block) + SIZEOF(.start_block);

SECTIONS {
    /* ### Picotool 'Binary Info' Entries
     *
     * Picotool looks through this block (as we have pointers to it in our
     * header) to find interesting information.
     */
    .bi_entries : ALIGN(4)
    {
        /* We put this in the header */
        __bi_entries_start = .;
        /* Here are the entries */
        KEEP(*(.bi_entries));
        /* Keep this block a nice round size */
        . = ALIGN(4);
        /* We put this in the header */
        __bi_entries_end = .;
    } > FLASH
} INSERT AFTER .text;

SECTIONS {
    /* ### USB MSC image
     *
     * Read-only FAT image exposed over USB mass storage.
     */
    .msc_image : ALIGN(4)
    {
        __msc_image_start = .;
        KEEP(*(.msc_image));
        __msc_image_end = .;
    } > MSC
} INSERT AFTER .text;

SECTIONS {
    /* ### Boot ROM extra info
     *
     * Goes after everything in our program, so it can contain a signature.
     */
    .end_block : ALIGN(4)
    {
        __end_block_addr = .;
        KEEP(*(.end_block));
    } > FLASH

} INSERT AFTER .uninit;

PROVIDE(start_to_end = __end_block_addr - __start_block_addr);
PROVIDE(end_to_start = __start_block_addr - __end_block_addr);
PROVIDE(__msc_start = ORIGIN(MSC));
PROVIDE(__msc_end = ORIGIN(MSC) + LENGTH(MSC));
PROVIDE(__persist_start = ORIGIN(PERSIST));
PROVIDE(__persist_end = ORIGIN(PERSIST) + LENGTH(PERSIST));
