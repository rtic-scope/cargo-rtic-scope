use anyhow::Result;
use nix::libc;
use std::fs;
use std::os::unix::io::AsRawFd;

mod ioctl {
    use super::libc;
    use nix::{ioctl_none_bad, ioctl_read_bad, ioctl_write_int_bad, ioctl_write_ptr_bad};

    ioctl_none_bad!(tiocexcl, libc::TIOCEXCL);
    ioctl_write_ptr_bad!(tcsetsw, libc::TCSETSW, libc::termios2);
    ioctl_read_bad!(tiocmget, libc::TIOCMGET, libc::c_int);
    ioctl_write_int_bad!(tiocmset, libc::TIOCMSET);
    ioctl_write_int_bad!(tcflsh, libc::TCFLSH);
}

pub fn configure(device: String) -> Result<fs::File> {
    let mut file = fs::OpenOptions::new().read(true).open(device)?;

    unsafe {
        let fd = file.as_raw_fd();

        // Put the serial port in exclusive mode. Further open(2) will
        // fail with EBUSY.
        ioctl::tiocexcl(fd)?;

        // Drain output buffer and configure serial port settings.
        ioctl::tcsetsw(
            fd,
            &libc::termios2 {
                c_iflag: 0x406,
                c_oflag: 0,
                c_cflag: 0x18b2,
                c_lflag: 0x8a30,
                c_line: 0,
                c_cc: [
                    0x03, 0x1c, 0x7f, 0x15, 0x04, 0x02, 0x64, 0x00, 0x11, 0x13, 0x1a, 0x00, 0x12,
                    0x0f, 0x17, 0x16, 0x00, 0x00, 0x00,
                ],
                c_ispeed: libc::B115200,
                c_ospeed: libc::B115200,
            },
        )?;
    }

    Ok(file)
}
