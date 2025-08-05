import { Entity, Column, PrimaryColumn, OneToMany, ManyToOne } from "typeorm";
import { File } from "./File";
import { Group } from "./Group";

@Entity()
export class User {
  @PrimaryColumn()
  uid: number;

  @Column()
  password: string;

  @Column()
  salt: string;

  @OneToMany(() => File, (file) => file.owner)
  files: File[];

  @ManyToOne(() => Group, (group) => group.users)
  group: Group;
}
