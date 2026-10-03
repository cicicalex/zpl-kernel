# Amprenta secvenței de pornire

Un kernel care se poartă la fel de la o versiune la alta ar trebui să spună
aceleași lucruri, în aceeași ordine. Fișierul ăsta ține valoarea curentă și
rețeta exactă prin care se calculează, fiindcă o valoare de referință a cărei
formulă nu e scrisă nicăieri nu e o referință: nimeni n-o poate reverifica.

## Valoarea curentă

```
39 markere non-SCHED
sha256 1befa4e71da907dc07626fe59f37d9b69974e7b72dbcf509e3a4fb3f684e98e1
```

Măsurată pe calea `-kernel` (QEMU, `zpl-kernel-bin`), care e singura
reproductibilă: calea ISO depinde de harta de memorie a mașinii, deci diferă
între `-m 256M` și `-m 512M` prin construcție, și asta e corect.

## Rețeta

Din logul serial al unei porniri:

1. se păstrează numai liniile care încep cu `[ZPL-`;
2. se aruncă cele care conțin `ZPL-ALIVE`, `rdtsc`, `ZPL-PERF` sau `ZPL-V04` —
   ticker, cronometre și meniu, adică lucruri care depind de ceas sau de taste;
3. se oprește după prima linie care conține `halt loop entered`, inclusiv;
4. se aruncă liniile `[ZPL-SCHED` — ordinea lor depinde de întreruperi;
5. `[ZPL-PDPT-VA=…]` se înlocuiește cu `[ZPL-PDPT-VA=MASKED]`, fiindcă e o
   adresă dată la legare;
6. se calculează `sha256` peste liniile rămase, unite cu `\n`, cu `\n` la final.

## Istoric

| valoare | de când | de ce s-a schimbat |
|---|---|---|
| `a7c50074fc3a5807…` | 1 octombrie 2026 | prima valoare scrisă undeva |
| `1befa4e71da907dc…` | 2 octombrie 2026 | markerul `[ZPL-FRAME] init` a încetat să fie un șir fix în cod și a început să măsoare. Textul lui s-a schimbat (`base=0x00200000`, `cap` din măsurătoare), deci și suma. Purtarea kernelului nu s-a schimbat: numărul de markere a rămas 39 și toate celelalte linii sunt identice. |

**Când se schimbă valoarea asta**, se schimbă și rândul de mai sus, cu motivul.
O amprentă care se actualizează tăcut nu mai apără nimic.
