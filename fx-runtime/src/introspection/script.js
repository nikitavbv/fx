document.addEventListener('click', (e) => {
  if (e.target.tagName === 'H2' && e.target.closest('section')) {
    e.target.closest('section').classList.toggle('collapsed');
  }
});
