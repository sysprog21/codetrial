pub struct A {
    pub next: Option<Box<B>>,
}

pub struct B {
    pub next: Option<Box<C>>,
}

pub struct C {
    pub next: Option<Box<D>>,
}

pub struct D {
    pub next: Option<Box<E>>,
}

pub struct E {
    pub next: Option<Box<F>>,
}

pub struct F {
    pub next: Option<Box<G>>,
}

pub struct G {
    pub next: Option<Box<H>>,
}

pub struct H {
    pub next: Option<Box<A>>,
}
