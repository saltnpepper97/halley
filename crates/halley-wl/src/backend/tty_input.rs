use super::*;
#[allow(clippy::type_complexity)]
pub(crate) fn build_tty_libinput_backend(
    session: Rc<RefCell<LibSeatSession>>,
    seat: &str,
) -> Result<(LibinputInputBackend, Rc<RefCell<Libinput>>), Box<dyn Error>> {
    let mut context = Libinput::new_with_udev(LibinputSessionInterface::from(session));
    context
        .udev_assign_seat(seat)
        .map_err(|_| io::Error::other(format!("libinput seat assign failed for {}", seat)))?;
    let context_handle = Rc::new(RefCell::new(context.clone()));
    Ok((LibinputInputBackend::new(context), context_handle))
}
